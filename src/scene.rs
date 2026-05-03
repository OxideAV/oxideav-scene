//! Root scene type.

use std::collections::BTreeMap;

use oxideav_core::{Rational, TimeBase};

use crate::audio::AudioCue;
use crate::duration::{SceneDuration, TimeStamp};
use crate::object::{Canvas, SceneObject};
use crate::page::Page;

/// Top-level scene — a canvas + a timeline of objects and audio cues.
///
/// A scene operates in one of two modes, mutually exclusive at
/// render-dispatch time:
///
/// 1. **Timeline mode** (`pages == None`): the existing model.
///    [`duration`](Self::duration) bounds the timeline, the renderer
///    samples objects at [`framerate`](Self::framerate). Used by the
///    streaming compositor (PNG / MP4 / RTMP), the NLE timeline, and
///    every raster-target writer.
/// 2. **Pages mode** (`pages == Some(_)`): the scene is a sequence
///    of [`Page`]s. Each page is independently sized + carries its
///    own [`oxideav_core::VectorFrame`]. Used by paged-content
///    writers (PDF, multi-page TIFF, EPUB). [`duration`](Self::duration)
///    + [`framerate`](Self::framerate) are ignored in this mode.
///
/// The two are NOT additive — a paged writer rejects a scene with
/// `pages == None`, and a video writer rejects one with
/// `pages == Some(_)`. Use [`Scene::pages_to_timeline`] /
/// [`Scene::timeline_to_pages`] to bridge across the modes.
#[derive(Clone, Debug)]
pub struct Scene {
    pub canvas: Canvas,
    pub duration: SceneDuration,
    /// Rational time base — all timestamps in the scene are integer
    /// multiples of this. Matches `oxideav-core`'s `TimeBase`.
    pub time_base: TimeBase,
    /// Output framerate. Separate from `time_base`: `time_base` sets
    /// the tick granularity of every scheduled event (keyframe,
    /// lifetime, audio cue trigger); `framerate` sets the cadence at
    /// which the renderer samples the scene and emits frames to a
    /// sink. A scene at `time_base = 1/1000` (ms) and `framerate =
    /// 30/1` renders at `t = 0, 33, 66, 100, …` ms. Videos included
    /// as `ObjectKind::Video` are retimed by the renderer so their
    /// per-frame PTS aligns with this cadence.
    pub framerate: Rational,
    /// Audio mix-bus sample rate.
    pub sample_rate: u32,
    pub background: Background,
    /// Z-ordered object list. Objects are composited in `z_order`
    /// ascending order; ties break by list position.
    pub objects: Vec<SceneObject>,
    pub audio: Vec<AudioCue>,
    pub metadata: Metadata,
    /// Paged-content sequence. `Some(...)` puts the scene into pages
    /// mode (PDF / multi-page TIFF / EPUB writers); `None` keeps it
    /// in timeline mode (the default). See [`Scene`] for the
    /// dispatch contract.
    pub pages: Option<Vec<Page>>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            canvas: Canvas::raster(1920, 1080),
            duration: SceneDuration::Finite(0),
            time_base: TimeBase::new(1, 1_000),
            framerate: Rational::new(30, 1),
            sample_rate: 48_000,
            background: Background::default(),
            objects: Vec::new(),
            audio: Vec::new(),
            metadata: Metadata::default(),
            pages: None,
        }
    }
}

impl Scene {
    /// Sort the object list by z-order, preserving insertion order
    /// within ties. Idempotent — call before rendering if the scene
    /// was built incrementally.
    pub fn sort_by_z_order(&mut self) {
        self.objects.sort_by_key(|o| o.z_order);
    }

    /// Convert a 0-based frame index to a scene-time timestamp.
    /// `frame_index / framerate = seconds`; multiplied by
    /// `time_base.den / time_base.num` to get scene-time ticks.
    /// Uses wide arithmetic to stay exact for the common rational
    /// values (24000/1001, 30000/1001, 60/1, …).
    pub fn frame_to_timestamp(&self, frame_index: u64) -> TimeStamp {
        let tb = self.time_base.0;
        let num = frame_index as i128 * self.framerate.den as i128 * tb.den as i128;
        let den = self.framerate.num as i128 * tb.num as i128;
        if den == 0 {
            0
        } else {
            (num / den) as TimeStamp
        }
    }

    /// Total frame count for finite scenes. `None` for
    /// `SceneDuration::Indefinite`. Rounds down — a 1000 ms scene
    /// at 30 fps yields 30 frames (0..=29), not 30.03.
    pub fn frame_count(&self) -> Option<u64> {
        let end = self.duration.end()?;
        let tb = self.time_base.0;
        if tb.num == 0 || self.framerate.den == 0 {
            return Some(0);
        }
        let num = (end as i128) * self.framerate.num as i128 * tb.num as i128;
        let den = self.framerate.den as i128 * tb.den as i128;
        if den == 0 {
            Some(0)
        } else {
            Some((num / den).max(0) as u64)
        }
    }

    /// Return objects live at time `t`, in paint order (z-order
    /// ascending).
    pub fn visible_at(&self, t: crate::duration::TimeStamp) -> Vec<&SceneObject> {
        let mut refs: Vec<&SceneObject> = self
            .objects
            .iter()
            .filter(|o| o.lifetime.is_live_at(t))
            .collect();
        refs.sort_by_key(|o| o.z_order);
        refs
    }

    /// Whether the scene is in pages mode. See the [`Scene`] doc
    /// comment for the contract.
    pub fn is_paged(&self) -> bool {
        self.pages.as_ref().is_some_and(|p| !p.is_empty())
    }

    /// Adapt a paged scene to a timeline by allotting
    /// `per_page_duration_ms` to each page sequentially. Returns a
    /// list of `(page_index, lifetime)` tuples — one per page —
    /// suitable for driving a video writer that needs a
    /// monotonically-advancing PTS axis.
    ///
    /// The returned timestamps are in the scene's `time_base`
    /// units. `per_page_duration_ms` is in milliseconds; the
    /// converter scales it via `time_base` so callers don't need to
    /// pre-convert.
    ///
    /// Returns an empty `Vec` when the scene is not in pages mode.
    pub fn pages_to_timeline(&self, per_page_duration_ms: u64) -> Vec<(usize, TimeStamp)> {
        let Some(ref pages) = self.pages else {
            return Vec::new();
        };
        // Convert ms → time_base ticks. time_base is num/den
        // seconds-per-tick; ticks per ms = den / (num * 1000).
        let tb = self.time_base.0;
        let num = (per_page_duration_ms as i128) * (tb.den as i128);
        let den = (tb.num as i128) * 1000;
        let ticks_per_page: TimeStamp = if den == 0 {
            0
        } else {
            (num / den) as TimeStamp
        };
        let mut out = Vec::with_capacity(pages.len());
        let mut t: TimeStamp = 0;
        for (i, _) in pages.iter().enumerate() {
            out.push((i, t));
            t = t.saturating_add(ticks_per_page);
        }
        out
    }

    /// Adapt a timeline-mode scene to discrete page-out points.
    /// Returns a `Vec<TimeStamp>` echoing `at_pts` filtered to
    /// in-range timestamps; the consumer renders one page per
    /// timestamp by sampling the scene at that PTS. Useful for
    /// "PDF preview every N seconds" workflows.
    ///
    /// Out-of-range PTS values (negative, or past the scene's end
    /// for finite scenes) are dropped — the renderer would refuse
    /// them anyway. Returns the input unchanged for indefinite
    /// scenes (modulo the `t >= 0` filter).
    pub fn timeline_to_pages(&self, at_pts: &[TimeStamp]) -> Vec<TimeStamp> {
        at_pts
            .iter()
            .copied()
            .filter(|&t| self.duration.contains(t))
            .collect()
    }
}

/// What fills the canvas below the z-ordered object list.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Background {
    /// Fully transparent. Export paths emit RGBA output.
    Transparent,
    /// Solid colour, `0xRRGGBBAA`.
    Solid(u32),
    /// Vertical or horizontal gradient between two colours.
    LinearGradient {
        from: u32,
        to: u32,
        /// Direction in degrees clockwise from 12 o'clock (0° = top,
        /// 90° = right, …).
        angle_deg: f32,
    },
    /// Bitmap background — cover or contain fit is up to the
    /// renderer's layout policy.
    Image(String),
}

impl Default for Background {
    fn default() -> Self {
        Background::Solid(0x000000FF)
    }
}

/// Scene-level metadata. Carried through to exports when the target
/// format supports it (PDF document info dict, MP4 `meta` box, etc).
///
/// `producer` and `creator` are intentionally distinct, mirroring
/// PDF's `/Info` dictionary:
///
/// - `creator` — the tool that authored the **source** content (e.g.
///   the NLE, drawing app, or word processor the user worked in).
/// - `producer` — the tool that wrote the **output** file (this
///   crate / oxideav exporter).
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Vec<String>,
    /// Authoring application — the tool used to create the source
    /// content. Distinct from [`producer`](Self::producer); PDF's
    /// `/Info` dictionary has separate `/Creator` and `/Producer`
    /// keys for the same reason.
    pub creator: Option<String>,
    /// Producing tool name — the writer that emitted the output
    /// file.
    pub producer: Option<String>,
    /// ISO-8601 string; not parsed here so exporters can pass it
    /// through unchanged.
    pub created_at: Option<String>,
    /// ISO-8601 modification timestamp. Mirrors `created_at`; PDF
    /// `/Info` has both `/CreationDate` and `/ModDate`, mp4 `mvhd`
    /// has both creation_time and modification_time, etc.
    pub modified_at: Option<String>,
    /// Extensible per-format extras. Lets callers carry metadata
    /// the standard fields don't cover: PDF `/Info` custom keys,
    /// Matroska `ContentTrack` tags, RDF properties, mp4 `udta`
    /// items not covered by the standard fields, ID3 frames, and
    /// so on. Keys are case-sensitive; uniqueness is the caller's
    /// responsibility (the map enforces it).
    pub custom: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{animation::Animation, duration::Lifetime, id::ObjectId, object::SceneObject};

    #[test]
    fn visible_at_respects_lifetime() {
        let mut scene = Scene {
            duration: SceneDuration::Finite(1000),
            ..Scene::default()
        };
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            lifetime: Lifetime {
                start: 100,
                end: Some(200),
            },
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(2),
            lifetime: Lifetime::default(),
            ..SceneObject::default()
        });
        let v = scene.visible_at(50);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, ObjectId::new(2));

        let v = scene.visible_at(150);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn frame_to_timestamp_30fps_1ms_tb() {
        let scene = Scene::default(); // 1/1000 tb, 30/1 fps
        assert_eq!(scene.frame_to_timestamp(0), 0);
        // 1 frame at 30 fps = 33.333 ms → floor = 33
        assert_eq!(scene.frame_to_timestamp(1), 33);
        assert_eq!(scene.frame_to_timestamp(30), 1000);
    }

    #[test]
    fn frame_to_timestamp_23_976_fps() {
        let scene = Scene {
            time_base: TimeBase::new(1, 90_000),
            framerate: Rational::new(24_000, 1001),
            ..Scene::default()
        };
        // Frame 24000 at 24000/1001 fps = 1001 seconds = 90090000 ticks.
        assert_eq!(scene.frame_to_timestamp(24_000), 90_090_000);
    }

    #[test]
    fn frame_count_finite() {
        let scene = Scene {
            duration: SceneDuration::Finite(1000), // 1 second at 1/1000 tb
            ..Scene::default()
        };
        assert_eq!(scene.frame_count(), Some(30));
    }

    #[test]
    fn frame_count_indefinite_is_none() {
        let scene = Scene {
            duration: SceneDuration::Indefinite,
            ..Scene::default()
        };
        assert_eq!(scene.frame_count(), None);
    }

    #[test]
    fn default_scene_is_timeline_mode() {
        let s = Scene::default();
        assert!(!s.is_paged());
        assert!(s.pages.is_none());
    }

    #[test]
    fn scene_in_pages_mode_reports_paged() {
        let s = Scene {
            pages: Some(vec![Page::new(595.0, 842.0), Page::new(842.0, 595.0)]),
            ..Scene::default()
        };
        assert!(s.is_paged());
        assert_eq!(s.pages.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn empty_pages_vec_is_not_paged() {
        let s = Scene {
            pages: Some(Vec::new()),
            ..Scene::default()
        };
        assert!(!s.is_paged());
    }

    #[test]
    fn pages_to_timeline_advances_per_page() {
        // 3 pages, 100 ms each, 1/1000 tb → ticks = 100 each.
        let s = Scene {
            pages: Some(vec![
                Page::new(595.0, 842.0),
                Page::new(595.0, 842.0),
                Page::new(595.0, 842.0),
            ]),
            ..Scene::default()
        };
        let tl = s.pages_to_timeline(100);
        assert_eq!(tl, vec![(0, 0), (1, 100), (2, 200)]);
    }

    #[test]
    fn pages_to_timeline_empty_for_timeline_scene() {
        let s = Scene::default();
        assert!(s.pages_to_timeline(100).is_empty());
    }

    #[test]
    fn pages_to_timeline_scales_for_90khz_tb() {
        // 1/90000 tb → 1 ms = 90 ticks; 100 ms = 9000 ticks.
        let s = Scene {
            time_base: TimeBase::new(1, 90_000),
            pages: Some(vec![Page::new(100.0, 100.0); 2]),
            ..Scene::default()
        };
        let tl = s.pages_to_timeline(100);
        assert_eq!(tl, vec![(0, 0), (1, 9_000)]);
    }

    #[test]
    fn timeline_to_pages_filters_out_of_range() {
        let s = Scene {
            duration: SceneDuration::Finite(1000),
            ..Scene::default()
        };
        let pts = vec![-1, 0, 500, 999, 1000, 5000];
        let kept = s.timeline_to_pages(&pts);
        assert_eq!(kept, vec![0, 500, 999]);
    }

    #[test]
    fn timeline_to_pages_indefinite_keeps_nonneg() {
        let s = Scene {
            duration: SceneDuration::Indefinite,
            ..Scene::default()
        };
        let pts = vec![-1, 0, i64::MAX];
        let kept = s.timeline_to_pages(&pts);
        assert_eq!(kept, vec![0, i64::MAX]);
    }

    #[test]
    fn metadata_default_is_empty() {
        let m = Metadata::default();
        assert!(m.title.is_none());
        assert!(m.creator.is_none());
        assert!(m.producer.is_none());
        assert!(m.created_at.is_none());
        assert!(m.modified_at.is_none());
        assert!(m.custom.is_empty());
    }

    #[test]
    fn metadata_custom_carries_extras() {
        let mut m = Metadata {
            creator: Some("MyDrawingApp 4.2".into()),
            producer: Some("oxideav-pdf 0.1".into()),
            modified_at: Some("2026-05-04T12:00:00Z".into()),
            ..Metadata::default()
        };
        m.custom
            .insert("dc:rights".into(), "(c) 2026 Karpeles Lab Inc.".into());
        m.custom.insert("Trapped".into(), "False".into());
        assert_eq!(m.creator.as_deref(), Some("MyDrawingApp 4.2"));
        assert_eq!(m.producer.as_deref(), Some("oxideav-pdf 0.1"));
        assert_eq!(m.modified_at.as_deref(), Some("2026-05-04T12:00:00Z"));
        assert_eq!(m.custom.get("Trapped").map(String::as_str), Some("False"));
        assert_eq!(m.custom.len(), 2);
    }

    #[test]
    fn sort_by_z_order_stable_ties() {
        let mut scene = Scene::default();
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            z_order: 5,
            animations: vec![Animation::new(
                crate::animation::AnimatedProperty::Opacity,
                Vec::new(),
                crate::animation::Easing::Linear,
                crate::animation::Repeat::Once,
            )],
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(2),
            z_order: 5,
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(3),
            z_order: 1,
            ..SceneObject::default()
        });
        scene.sort_by_z_order();
        assert_eq!(scene.objects[0].id, ObjectId::new(3));
        assert_eq!(scene.objects[1].id, ObjectId::new(1));
        assert_eq!(scene.objects[2].id, ObjectId::new(2));
    }
}
