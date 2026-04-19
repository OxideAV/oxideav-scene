//! Root scene type.

use oxideav_core::{Rational, TimeBase};

use crate::audio::AudioCue;
use crate::duration::{SceneDuration, TimeStamp};
use crate::object::{Canvas, SceneObject};

/// Top-level scene — a canvas + a timeline of objects and audio cues.
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
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Vec<String>,
    /// Producing tool name.
    pub producer: Option<String>,
    /// ISO-8601 string; not parsed here so exporters can pass it
    /// through unchanged.
    pub created_at: Option<String>,
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
