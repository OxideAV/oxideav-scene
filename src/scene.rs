//! Root scene type.

use std::collections::BTreeMap;
use std::sync::Arc;

use oxideav_core::{Rational, TimeBase, VideoFrame};

use crate::audio::AudioCue;
use crate::duration::{Lifetime, SceneDuration, TimeStamp};
use crate::id::ObjectId;
use crate::light::LightInstance;
use crate::object::{Canvas, SceneObject};
use crate::ops::Operation;
use crate::page::Page;
use crate::paint::Gradient;

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
    /// 3D punctual lights for scenes that carry 3D content.
    ///
    /// Each [`LightInstance`] pairs a typed [`crate::Light`] with its
    /// world-space position and emission direction. The list is the
    /// typed landing place for 3D-scene readers (glTF, USD, OBJ
    /// importers) and the typed source for 3D-scene writers. The
    /// 2D `RasterRenderer` (the vector-slice driver) ignores this
    /// list entirely — light contribution to raster composition is
    /// a separate follow-up.
    ///
    /// Empty by default. Ordering is preserved for round-trippability
    /// but has no rendering semantics (lights are commutative under
    /// the linear sum the renderer performs); writers may sort or
    /// reorder freely.
    pub lights: Vec<LightInstance>,
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
            lights: Vec::new(),
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

    /// Apply one [`Operation`] to the scene. Returns a short
    /// human-readable receipt describing what happened — useful for
    /// streaming-compositor logs and for tests asserting on the
    /// effect of a control-plane message.
    ///
    /// The semantics:
    ///
    /// - [`Operation::AddObject`] — appends the object and re-sorts
    ///   by z-order. Returns `"add obj#N"`.
    /// - [`Operation::RemoveObject`] — removes the object with the
    ///   given id. `at` is informational here: the in-process driver
    ///   removes immediately; a future scheduling layer can read `at`
    ///   off the [`Operation::RemoveObject`] variant to defer.
    /// - [`Operation::SetTransform`] — overwrites the base transform.
    ///   Animations on the same object continue to add to this base.
    /// - [`Operation::Animate`] — appends the animation; no
    ///   replacement of existing tracks on the same property (use
    ///   [`Operation::CancelAnimation`] first).
    /// - [`Operation::CancelAnimation`] — removes the *first*
    ///   animation matching the property.
    /// - [`Operation::FireAudio`] — appends the cue to the scene's
    ///   audio bus.
    /// - [`Operation::EndScene`] — switches a streaming scene to
    ///   `SceneDuration::Finite(now)`. The caller passes the current
    ///   scene-time clock via a future signature once a clock source
    ///   exists; for now the receipt notes that the operation was
    ///   recorded but the duration stays unchanged.
    ///
    /// Returns `Err(&'static str)` only when the operation targets a
    /// non-existent object id (Remove / SetTransform / Animate /
    /// CancelAnimation); all other operations succeed unconditionally.
    pub fn apply(&mut self, op: Operation) -> Result<String, &'static str> {
        match op {
            Operation::AddObject(obj) => {
                let id = obj.id;
                self.objects.push(*obj);
                self.sort_by_z_order();
                Ok(format!("add {id}"))
            }
            Operation::RemoveObject { id, at: _ } => {
                let before = self.objects.len();
                self.objects.retain(|o| o.id != id);
                if self.objects.len() == before {
                    Err("object id not found")
                } else {
                    Ok(format!("remove {id}"))
                }
            }
            Operation::SetTransform { id, transform } => {
                let obj = self
                    .objects
                    .iter_mut()
                    .find(|o| o.id == id)
                    .ok_or("object id not found")?;
                obj.transform = transform;
                Ok(format!("set-transform {id}"))
            }
            Operation::Animate { id, animation } => {
                let obj = self
                    .objects
                    .iter_mut()
                    .find(|o| o.id == id)
                    .ok_or("object id not found")?;
                obj.animations.push(animation);
                Ok(format!("animate {id}"))
            }
            Operation::CancelAnimation { id, property } => {
                let obj = self
                    .objects
                    .iter_mut()
                    .find(|o| o.id == id)
                    .ok_or("object id not found")?;
                let len_before = obj.animations.len();
                if let Some(idx) = obj.animations.iter().position(|a| a.property == property) {
                    obj.animations.remove(idx);
                }
                if obj.animations.len() == len_before {
                    Ok(format!("cancel-animation {id} (no match)"))
                } else {
                    Ok(format!("cancel-animation {id}"))
                }
            }
            Operation::FireAudio(cue) => {
                self.audio.push(*cue);
                Ok("fire-audio".to_string())
            }
            Operation::EndScene => Ok("end-scene".to_string()),
        }
    }

    /// Apply a batch of operations sequentially. Stops at the first
    /// error and returns the receipts gathered so far in the `Err`
    /// arm's second element. Useful for replaying a recorded
    /// control-plane log onto a fresh scene.
    pub fn apply_batch(
        &mut self,
        ops: impl IntoIterator<Item = Operation>,
    ) -> Result<Vec<String>, (Vec<String>, &'static str)> {
        let mut receipts = Vec::new();
        for op in ops {
            match self.apply(op) {
                Ok(r) => receipts.push(r),
                Err(e) => return Err((receipts, e)),
            }
        }
        Ok(receipts)
    }

    /// Splice another scene into this one. Used by NLE-style
    /// compose-track-then-append workflows: prepare scene B against
    /// a known time origin, then merge it onto scene A at a chosen
    /// offset.
    ///
    /// Semantics:
    ///
    /// - **Objects** — every object from `other` is appended with its
    ///   `lifetime` shifted by `time_offset` (lifetimes' `end` is
    ///   only shifted when `Some`; `None` lifetimes stay open-ended)
    ///   and its `z_order` offset by `z_offset` so it stacks above
    ///   (or below) the existing objects without colliding.
    /// - **Audio cues** — every cue from `other` is appended with
    ///   its `trigger` shifted by `time_offset`.
    /// - **Object ids** — preserved verbatim. If the caller wants
    ///   uniqueness across the merged scene, they remap ids before
    ///   calling.
    /// - **Canvas, framerate, duration, metadata, pages, background**
    ///   — `self`'s wins; nothing from `other` overrides.
    /// - **Duration** — for finite scenes, `self.duration` is
    ///   extended to cover any shifted lifetime that reaches past
    ///   the current end.
    ///
    /// Returns the number of objects + cues appended.
    pub fn merge(
        &mut self,
        other: &Scene,
        time_offset: TimeStamp,
        z_offset: i32,
    ) -> (usize, usize) {
        let n_obj = other.objects.len();
        let n_cue = other.audio.len();

        for obj in &other.objects {
            let mut shifted = obj.clone();
            shifted.lifetime = Lifetime {
                start: obj.lifetime.start.saturating_add(time_offset),
                end: obj.lifetime.end.map(|e| e.saturating_add(time_offset)),
            };
            shifted.z_order = obj.z_order.saturating_add(z_offset);
            // Shift animation keyframe times so timing stays correct
            // relative to the new origin.
            for anim in shifted.animations.iter_mut() {
                for kf in anim.keyframes.iter_mut() {
                    kf.time = kf.time.saturating_add(time_offset);
                }
            }
            self.objects.push(shifted);
        }

        for cue in &other.audio {
            let mut shifted = cue.clone();
            shifted.trigger = cue.trigger.saturating_add(time_offset);
            self.audio.push(shifted);
        }

        // Lights have no timeline component yet — copy verbatim.
        // Per-light animation is a follow-up; for now, the world-space
        // pose is constant for the lifetime of the scene.
        self.lights.extend(other.lights.iter().cloned());

        // Extend our duration to cover any reach past the current end
        // for finite scenes. Indefinite stays indefinite.
        if let SceneDuration::Finite(end) = self.duration {
            let mut new_end = end;
            for obj in &other.objects {
                if let Some(other_end) = obj.lifetime.end {
                    let candidate = other_end.saturating_add(time_offset);
                    if candidate > new_end {
                        new_end = candidate;
                    }
                }
            }
            if let SceneDuration::Finite(other_end) = other.duration {
                let candidate = other_end.saturating_add(time_offset);
                if candidate > new_end {
                    new_end = candidate;
                }
            }
            if new_end != end {
                self.duration = SceneDuration::Finite(new_end);
            }
        }

        self.sort_by_z_order();
        (n_obj, n_cue)
    }

    /// Allocate a fresh [`ObjectId`] guaranteed not to collide with
    /// any existing object in the scene. Implementation: `max(id) +
    /// 1` (with `0` reserved for the sentinel). Cheap O(N) — call
    /// once per add, not in a tight loop.
    pub fn next_object_id(&self) -> ObjectId {
        let max = self.objects.iter().map(|o| o.id.raw()).max().unwrap_or(0);
        ObjectId::new(max.saturating_add(1).max(1))
    }

    /// Append a [`LightInstance`] to [`Self::lights`] and return its
    /// index in the list. Convenience for incremental scene
    /// construction.
    pub fn push_light(&mut self, instance: LightInstance) -> usize {
        let idx = self.lights.len();
        self.lights.push(instance);
        idx
    }

    /// `true` when [`Self::lights`] has at least one entry.
    pub fn has_lights(&self) -> bool {
        !self.lights.is_empty()
    }

    /// Iterate over every [`LightInstance`] of a given variant
    /// (selected by the matching predicate). Use with the variant
    /// predicates from [`crate::Light`]:
    ///
    /// ```
    /// use oxideav_scene::Scene;
    /// use oxideav_scene::light::{Light, LightCommon, LightInstance};
    /// let mut s = Scene::default();
    /// s.push_light(LightInstance::new(Light::Point {
    ///     common: LightCommon::default(),
    /// }));
    /// let point_count = s.lights_filter(Light::is_point).count();
    /// assert_eq!(point_count, 1);
    /// ```
    pub fn lights_filter<F>(&self, mut predicate: F) -> impl Iterator<Item = &LightInstance>
    where
        F: FnMut(&crate::light::Light) -> bool,
    {
        self.lights
            .iter()
            .filter(move |inst| predicate(&inst.light))
    }

    /// Union axis-aligned bounding box of every object live at time
    /// `t`, in canvas space. `None` when no object is live at `t`.
    ///
    /// Per-object bounds are computed via
    /// [`SceneObject::bbox`](crate::SceneObject::bbox) — see that
    /// method for the content-size lookup + clip semantics.
    /// `fallback` is forwarded verbatim for objects whose kind
    /// doesn't expose an intrinsic content size (raster images,
    /// video frames, text runs); pass the canvas dims (or any
    /// per-object hint the caller has) so those objects still
    /// contribute a sensible AABB.
    ///
    /// The union is the smallest axis-aligned rectangle enclosing
    /// every contributing object box. Clipped objects with an empty
    /// intersection (zero extent) are skipped — they would otherwise
    /// drag the union to their min corner without contributing
    /// useful coverage. Object opacity, [`BlendMode`](crate::BlendMode),
    /// and [`Effect`](crate::Effect) chains are NOT considered:
    /// `bbox_at` reports the *geometric* footprint, not the
    /// *visible* one.
    pub fn bbox_at(
        &self,
        t: crate::duration::TimeStamp,
        fallback: (f32, f32),
    ) -> Option<oxideav_core::Rect> {
        let mut acc: Option<(f32, f32, f32, f32)> = None;
        for o in &self.objects {
            if !o.lifetime.is_live_at(t) {
                continue;
            }
            let bb = o.bbox(fallback);
            // Skip clipped-out objects so they don't pull the union
            // toward their clip corner.
            if bb.width <= 0.0 || bb.height <= 0.0 {
                continue;
            }
            let (x1, y1, x2, y2) = (bb.x, bb.y, bb.x + bb.width, bb.y + bb.height);
            acc = Some(match acc {
                None => (x1, y1, x2, y2),
                Some((mx1, my1, mx2, my2)) => (mx1.min(x1), my1.min(y1), mx2.max(x2), my2.max(y2)),
            });
        }
        acc.map(|(x1, y1, x2, y2)| oxideav_core::Rect::new(x1, y1, x2 - x1, y2 - y1))
    }

    /// Identify the top-most object whose axis-aligned bounding box
    /// (per [`SceneObject::bbox`](crate::SceneObject::bbox)) contains
    /// `point` at scene time `t`, returning its [`ObjectId`].
    ///
    /// "Top-most" follows the painter's algorithm: higher `z_order`
    /// wins, ties broken by later position in
    /// [`Scene::objects`](Self::objects) (i.e. last-added on top).
    /// Returns `None` when no live object's AABB contains `point`.
    ///
    /// This is a *bounding-box* hit test, not a per-pixel shape hit
    /// test. A rotated rectangle's AABB contains corners that the
    /// rectangle itself does not — callers needing pixel-accurate
    /// picking (e.g. for clicking on a polygon hole) should follow
    /// this up with a per-shape contains check. Hit-testing through
    /// transparent regions of a bitmap is similarly not handled
    /// here — the AABB is the geometric footprint, opacity is
    /// ignored.
    ///
    /// `fallback` is the content size used for kinds without an
    /// intrinsic extent (raster images, video, text). The caller
    /// supplies it for the same reasons as
    /// [`Scene::bbox_at`](Self::bbox_at).
    pub fn hit_test_at(
        &self,
        t: crate::duration::TimeStamp,
        point: oxideav_core::Point,
        fallback: (f32, f32),
    ) -> Option<ObjectId> {
        // Walk all candidates with their original index so the
        // tie-break (later index wins at equal z_order) is exact.
        let mut hit: Option<(i32, usize, ObjectId)> = None;
        for (idx, o) in self.objects.iter().enumerate() {
            if !o.lifetime.is_live_at(t) {
                continue;
            }
            let bb = o.bbox(fallback);
            if bb.width <= 0.0 || bb.height <= 0.0 {
                continue;
            }
            if point.x < bb.x
                || point.x > bb.x + bb.width
                || point.y < bb.y
                || point.y > bb.y + bb.height
            {
                continue;
            }
            let cand = (o.z_order, idx, o.id);
            hit = Some(match hit {
                None => cand,
                Some(prev) => {
                    if cand.0 > prev.0 || (cand.0 == prev.0 && cand.1 > prev.1) {
                        cand
                    } else {
                        prev
                    }
                }
            });
        }
        hit.map(|(_, _, id)| id)
    }

    /// Sample every object live at time `t` and return the resolved
    /// per-object state in paint order (z-order ascending; ties broken
    /// by insertion order, matching
    /// [`visible_at`](Self::visible_at)).
    ///
    /// Each returned [`crate::object::Sample`] carries the object's
    /// composed transform + opacity + forwarded compositor fields, so
    /// a renderer can iterate the result, look the matching
    /// [`SceneObject`] up by `id` for its `kind` payload, and feed
    /// state straight into the painter — no further animation
    /// evaluation needed.
    ///
    /// See [`SceneObject::sample_at`] for the per-object composition
    /// rule. The returned [`Vec`] is empty when no object is live at
    /// `t`.
    pub fn sampled_at(&self, t: crate::duration::TimeStamp) -> Vec<crate::object::Sample> {
        let mut refs: Vec<&SceneObject> = self
            .objects
            .iter()
            .filter(|o| o.lifetime.is_live_at(t))
            .collect();
        refs.sort_by_key(|o| o.z_order);
        refs.into_iter().map(|o| o.sample_at(t)).collect()
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
    ///
    /// Kept for the common two-colour case. For richer multi-stop
    /// gradients, use [`Background::Gradient`] which carries a full
    /// [`Gradient`] (linear or radial, any number of stops).
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
    /// Pre-decoded straight-alpha RGBA8 bitmap background — symmetric
    /// with [`crate::ImageSource::Decoded`] on the object side. The
    /// carried [`oxideav_core::VideoFrame`] is read under the same
    /// canonical RGBA8-stride convention the rest of the scene crate
    /// uses (`width = stride / 4`,
    /// `height = data.len() / stride`), so a frame produced by
    /// `oxideav_raster::Renderer::render` (or any other RGBA8 source
    /// emitting that convention) can be dropped straight in as a
    /// backdrop without invoking a decoder.
    ///
    /// The renderer composites it full-canvas: the source frame is
    /// drawn into the canvas-sized rectangle `(0, 0)..(w, h)` via the
    /// downstream rasteriser's image sampler (bilinear by default), so
    /// a non-canvas-sized backdrop is rescaled to cover. Future
    /// renderer revisions may add explicit cover / contain / tile
    /// layout policies; until then the canvas-fill behaviour matches
    /// the simplest "stretch to backdrop" interpretation.
    ///
    /// The path-based [`Background::Image`] variant continues to skip
    /// silently — pre-decode upstream and feed the resulting frame
    /// back in via this variant until a decoder-aware renderer lands.
    DecodedImage(Arc<VideoFrame>),
    /// Rich gradient background — multi-stop linear or radial. See
    /// the [`crate::paint`] module for stop conventions; the same
    /// per-channel linear interpolation as
    /// [`crate::animation::KeyframeValue::Color`] is used by the
    /// gradient shader.
    Gradient(Gradient),
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
    use crate::{
        animation::{AnimatedProperty, Animation, Easing, Keyframe, KeyframeValue, Repeat},
        audio::{AudioCue, AudioSource, Generator},
        id::ObjectId,
        object::{SceneObject, Transform},
        ops::Operation,
        paint::{Gradient, Stop},
    };

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
    fn apply_add_then_remove_roundtrip() {
        let mut s = Scene::default();
        let id = ObjectId::new(7);
        let receipt = s
            .apply(Operation::AddObject(Box::new(SceneObject {
                id,
                ..SceneObject::default()
            })))
            .unwrap();
        assert!(receipt.starts_with("add "));
        assert_eq!(s.objects.len(), 1);

        let receipt = s.apply(Operation::RemoveObject { id, at: 0 }).unwrap();
        assert!(receipt.starts_with("remove "));
        assert!(s.objects.is_empty());
    }

    #[test]
    fn apply_remove_unknown_id_errs() {
        let mut s = Scene::default();
        let err = s
            .apply(Operation::RemoveObject {
                id: ObjectId::new(99),
                at: 0,
            })
            .unwrap_err();
        assert_eq!(err, "object id not found");
    }

    #[test]
    fn apply_set_transform_overwrites_base() {
        let mut s = Scene::default();
        let id = ObjectId::new(3);
        s.objects.push(SceneObject {
            id,
            ..SceneObject::default()
        });
        let new_t = Transform {
            position: (100.0, 50.0),
            ..Transform::identity()
        };
        s.apply(Operation::SetTransform {
            id,
            transform: new_t,
        })
        .unwrap();
        assert_eq!(s.objects[0].transform.position, (100.0, 50.0));
    }

    #[test]
    fn apply_animate_appends_track() {
        let mut s = Scene::default();
        let id = ObjectId::new(11);
        s.objects.push(SceneObject {
            id,
            ..SceneObject::default()
        });
        let anim = Animation::new(
            AnimatedProperty::Opacity,
            vec![
                Keyframe {
                    time: 0,
                    value: KeyframeValue::Scalar(0.0),
                    easing: None,
                },
                Keyframe {
                    time: 100,
                    value: KeyframeValue::Scalar(1.0),
                    easing: None,
                },
            ],
            Easing::Linear,
            Repeat::Once,
        );
        s.apply(Operation::Animate {
            id,
            animation: anim,
        })
        .unwrap();
        assert_eq!(s.objects[0].animations.len(), 1);
    }

    #[test]
    fn apply_cancel_animation_removes_first_match() {
        let mut s = Scene::default();
        let id = ObjectId::new(11);
        let anim = Animation::new(
            AnimatedProperty::Opacity,
            vec![Keyframe {
                time: 0,
                value: KeyframeValue::Scalar(0.0),
                easing: None,
            }],
            Easing::Linear,
            Repeat::Once,
        );
        s.objects.push(SceneObject {
            id,
            animations: vec![anim],
            ..SceneObject::default()
        });
        let receipt = s
            .apply(Operation::CancelAnimation {
                id,
                property: AnimatedProperty::Opacity,
            })
            .unwrap();
        assert!(receipt.starts_with("cancel-animation "));
        assert!(!receipt.contains("(no match)"));
        assert!(s.objects[0].animations.is_empty());
    }

    #[test]
    fn apply_cancel_animation_no_match_reports_silently() {
        let mut s = Scene::default();
        let id = ObjectId::new(11);
        s.objects.push(SceneObject {
            id,
            ..SceneObject::default()
        });
        let receipt = s
            .apply(Operation::CancelAnimation {
                id,
                property: AnimatedProperty::Opacity,
            })
            .unwrap();
        assert!(receipt.contains("(no match)"));
    }

    #[test]
    fn apply_fire_audio_appends_cue() {
        let mut s = Scene::default();
        s.apply(Operation::FireAudio(Box::new(AudioCue {
            trigger: 500,
            source: AudioSource::Generator(Generator::Silence),
            volume: Animation::new(
                AnimatedProperty::Volume,
                Vec::new(),
                Easing::Linear,
                Repeat::Once,
            ),
            duck: Vec::new(),
            end: None,
        })))
        .unwrap();
        assert_eq!(s.audio.len(), 1);
        assert_eq!(s.audio[0].trigger, 500);
    }

    #[test]
    fn apply_batch_collects_receipts() {
        let mut s = Scene::default();
        let id = ObjectId::new(2);
        let receipts = s
            .apply_batch([
                Operation::AddObject(Box::new(SceneObject {
                    id,
                    ..SceneObject::default()
                })),
                Operation::SetTransform {
                    id,
                    transform: Transform::identity(),
                },
                Operation::EndScene,
            ])
            .unwrap();
        assert_eq!(receipts.len(), 3);
    }

    #[test]
    fn apply_batch_stops_on_first_error() {
        let mut s = Scene::default();
        let (receipts, err) = s
            .apply_batch([
                Operation::EndScene,
                Operation::RemoveObject {
                    id: ObjectId::new(404),
                    at: 0,
                },
                Operation::EndScene, // never reached
            ])
            .unwrap_err();
        assert_eq!(receipts.len(), 1);
        assert_eq!(err, "object id not found");
    }

    #[test]
    fn merge_shifts_lifetimes_and_extends_duration() {
        let mut a = Scene {
            duration: SceneDuration::Finite(1000),
            ..Scene::default()
        };
        a.objects.push(SceneObject {
            id: ObjectId::new(1),
            lifetime: Lifetime {
                start: 0,
                end: Some(500),
            },
            z_order: 0,
            ..SceneObject::default()
        });

        let mut b = Scene {
            duration: SceneDuration::Finite(800),
            ..Scene::default()
        };
        b.objects.push(SceneObject {
            id: ObjectId::new(10),
            lifetime: Lifetime {
                start: 0,
                end: Some(400),
            },
            z_order: 0,
            ..SceneObject::default()
        });

        let (n_obj, n_cue) = a.merge(&b, 800, 100);
        assert_eq!(n_obj, 1);
        assert_eq!(n_cue, 0);
        assert_eq!(a.objects.len(), 2);
        // b had duration 800, shifted by 800 → 1600. b's object
        // lifetime end is 400 + 800 = 1200, but b's own duration
        // dominates. a's duration was 1000 → must extend to 1600.
        assert_eq!(a.duration, SceneDuration::Finite(1600));
        // z-order offset applied.
        let merged = &a.objects.iter().find(|o| o.id.raw() == 10).unwrap();
        assert_eq!(merged.z_order, 100);
        assert_eq!(merged.lifetime.start, 800);
        assert_eq!(merged.lifetime.end, Some(1200));
    }

    #[test]
    fn merge_shifts_animation_keyframes() {
        let mut a = Scene::default();
        let mut b = Scene::default();
        let anim = Animation::new(
            AnimatedProperty::Opacity,
            vec![
                Keyframe {
                    time: 0,
                    value: KeyframeValue::Scalar(0.0),
                    easing: None,
                },
                Keyframe {
                    time: 100,
                    value: KeyframeValue::Scalar(1.0),
                    easing: None,
                },
            ],
            Easing::Linear,
            Repeat::Once,
        );
        b.objects.push(SceneObject {
            id: ObjectId::new(5),
            animations: vec![anim],
            ..SceneObject::default()
        });
        a.merge(&b, 1_000, 0);
        let merged = a.objects.iter().find(|o| o.id.raw() == 5).unwrap();
        assert_eq!(merged.animations[0].keyframes[0].time, 1_000);
        assert_eq!(merged.animations[0].keyframes[1].time, 1_100);
    }

    #[test]
    fn merge_audio_cues_shift_too() {
        let mut a = Scene::default();
        let mut b = Scene::default();
        b.audio.push(AudioCue {
            trigger: 250,
            source: AudioSource::Generator(Generator::Silence),
            volume: Animation::new(
                AnimatedProperty::Volume,
                Vec::new(),
                Easing::Linear,
                Repeat::Once,
            ),
            duck: Vec::new(),
            end: None,
        });
        let (_, n_cue) = a.merge(&b, 500, 0);
        assert_eq!(n_cue, 1);
        assert_eq!(a.audio[0].trigger, 750);
    }

    #[test]
    fn next_object_id_avoids_collisions() {
        let mut s = Scene::default();
        assert_eq!(s.next_object_id().raw(), 1);
        s.objects.push(SceneObject {
            id: ObjectId::new(5),
            ..SceneObject::default()
        });
        s.objects.push(SceneObject {
            id: ObjectId::new(42),
            ..SceneObject::default()
        });
        assert_eq!(s.next_object_id().raw(), 43);
    }

    #[test]
    fn background_gradient_carries_full_stops() {
        let bg = Background::Gradient(Gradient::linear(
            45.0,
            vec![
                Stop::new(0.0, 0xFF0000FF),
                Stop::new(0.5, 0x00FF00FF),
                Stop::new(1.0, 0x0000FFFF),
            ],
        ));
        if let Background::Gradient(g) = bg {
            assert_eq!(g.stops().len(), 3);
        } else {
            panic!("expected Background::Gradient");
        }
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

    // ---- bbox_at / hit_test_at -----------------------------------

    use crate::object::{ClipRect, ObjectKind, Shape};

    fn rect_obj(id: u64, x: f32, y: f32, w: f32, h: f32, z: i32) -> SceneObject {
        SceneObject {
            id: ObjectId::new(id),
            kind: ObjectKind::Shape(Shape::Rect {
                width: w,
                height: h,
                fill: 0,
                stroke: None,
                corner_radius: 0.0,
            }),
            transform: Transform {
                position: (x, y),
                ..Transform::identity()
            },
            z_order: z,
            ..SceneObject::default()
        }
    }

    #[test]
    fn bbox_at_returns_none_for_empty_scene() {
        let s = Scene::default();
        assert!(s.bbox_at(0, (1920.0, 1080.0)).is_none());
    }

    #[test]
    fn bbox_at_returns_union_of_live_object_aabbs() {
        let mut s = Scene::default();
        s.objects.push(rect_obj(1, 0.0, 0.0, 10.0, 10.0, 0));
        s.objects.push(rect_obj(2, 50.0, 80.0, 20.0, 20.0, 0));
        let bb = s.bbox_at(0, (0.0, 0.0)).unwrap();
        // Union spans x ∈ [0, 70], y ∈ [0, 100].
        assert!((bb.x - 0.0).abs() < 1e-4);
        assert!((bb.y - 0.0).abs() < 1e-4);
        assert!((bb.width - 70.0).abs() < 1e-4);
        assert!((bb.height - 100.0).abs() < 1e-4);
    }

    #[test]
    fn bbox_at_skips_dead_objects() {
        let mut s = Scene {
            duration: SceneDuration::Finite(1000),
            ..Scene::default()
        };
        let mut alive = rect_obj(1, 0.0, 0.0, 10.0, 10.0, 0);
        alive.lifetime = Lifetime::default(); // always live
        s.objects.push(alive);
        let mut dead = rect_obj(2, 500.0, 500.0, 100.0, 100.0, 0);
        dead.lifetime = Lifetime {
            start: 500,
            end: Some(600),
        };
        s.objects.push(dead);
        // At t=10 only the always-live object contributes; the
        // dead one would have stretched the union to (600, 600).
        let bb = s.bbox_at(10, (0.0, 0.0)).unwrap();
        assert!((bb.width - 10.0).abs() < 1e-4);
        assert!((bb.height - 10.0).abs() < 1e-4);
    }

    #[test]
    fn bbox_at_skips_clipped_out_objects() {
        let mut s = Scene::default();
        s.objects.push(rect_obj(1, 0.0, 0.0, 10.0, 10.0, 0));
        let mut clipped = rect_obj(2, 0.0, 0.0, 100.0, 100.0, 0);
        // Clip rect that misses the object entirely.
        clipped.clip = Some(ClipRect {
            x: 9999.0,
            y: 9999.0,
            width: 1.0,
            height: 1.0,
        });
        s.objects.push(clipped);
        let bb = s.bbox_at(0, (0.0, 0.0)).unwrap();
        assert!((bb.width - 10.0).abs() < 1e-4);
        assert!((bb.height - 10.0).abs() < 1e-4);
    }

    #[test]
    fn hit_test_at_returns_top_object_under_point() {
        let mut s = Scene::default();
        s.objects.push(rect_obj(1, 0.0, 0.0, 100.0, 100.0, 0));
        s.objects.push(rect_obj(2, 0.0, 0.0, 100.0, 100.0, 5));
        s.objects.push(rect_obj(3, 0.0, 0.0, 100.0, 100.0, 2));
        let hit = s.hit_test_at(0, oxideav_core::Point::new(50.0, 50.0), (0.0, 0.0));
        assert_eq!(hit, Some(ObjectId::new(2)));
    }

    #[test]
    fn hit_test_at_ties_break_by_insertion_order() {
        let mut s = Scene::default();
        s.objects.push(rect_obj(1, 0.0, 0.0, 100.0, 100.0, 5));
        s.objects.push(rect_obj(2, 0.0, 0.0, 100.0, 100.0, 5));
        let hit = s.hit_test_at(0, oxideav_core::Point::new(10.0, 10.0), (0.0, 0.0));
        // Later-added wins at equal z_order.
        assert_eq!(hit, Some(ObjectId::new(2)));
    }

    #[test]
    fn hit_test_at_misses_when_no_object_under_point() {
        let mut s = Scene::default();
        s.objects.push(rect_obj(1, 0.0, 0.0, 10.0, 10.0, 0));
        let hit = s.hit_test_at(0, oxideav_core::Point::new(500.0, 500.0), (0.0, 0.0));
        assert!(hit.is_none());
    }

    #[test]
    fn hit_test_at_ignores_dead_objects() {
        let mut s = Scene::default();
        let mut dead = rect_obj(1, 0.0, 0.0, 100.0, 100.0, 0);
        dead.lifetime = Lifetime {
            start: 100,
            end: Some(200),
        };
        s.objects.push(dead);
        // t = 0 → object is not yet live.
        let hit = s.hit_test_at(0, oxideav_core::Point::new(50.0, 50.0), (0.0, 0.0));
        assert!(hit.is_none());
    }

    #[test]
    fn sampled_at_returns_empty_for_empty_scene() {
        let s = Scene::default();
        assert!(s.sampled_at(0).is_empty());
    }

    #[test]
    fn sampled_at_returns_paint_order_z_ascending() {
        let mut s = Scene::default();
        s.objects.push(rect_obj(1, 0.0, 0.0, 10.0, 10.0, 5));
        s.objects.push(rect_obj(2, 0.0, 0.0, 10.0, 10.0, 1));
        s.objects.push(rect_obj(3, 0.0, 0.0, 10.0, 10.0, 3));
        let samples = s.sampled_at(0);
        // Paint-order = z ascending: ids 2, 3, 1.
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].id, ObjectId::new(2));
        assert_eq!(samples[1].id, ObjectId::new(3));
        assert_eq!(samples[2].id, ObjectId::new(1));
    }

    #[test]
    fn sampled_at_skips_dead_objects() {
        let mut s = Scene {
            duration: SceneDuration::Finite(1000),
            ..Scene::default()
        };
        let mut dead = rect_obj(1, 0.0, 0.0, 10.0, 10.0, 0);
        dead.lifetime = Lifetime {
            start: 500,
            end: Some(600),
        };
        s.objects.push(dead);
        s.objects.push(rect_obj(2, 0.0, 0.0, 10.0, 10.0, 0));
        let samples = s.sampled_at(0);
        // Dead one (1) is skipped; only (2) is live.
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].id, ObjectId::new(2));
    }

    #[test]
    fn sampled_at_picks_up_per_object_animation_state() {
        let mut s = Scene::default();
        let id = ObjectId::new(7);
        s.objects.push(SceneObject {
            id,
            kind: ObjectKind::Shape(Shape::Rect {
                width: 10.0,
                height: 10.0,
                fill: 0,
                stroke: None,
                corner_radius: 0.0,
            }),
            transform: Transform {
                position: (1.0, 2.0),
                ..Transform::identity()
            },
            animations: vec![Animation::new(
                AnimatedProperty::Position,
                vec![
                    Keyframe {
                        time: 0,
                        value: KeyframeValue::Vec2(10.0, 20.0),
                        easing: None,
                    },
                    Keyframe {
                        time: 100,
                        value: KeyframeValue::Vec2(10.0, 20.0),
                        easing: None,
                    },
                ],
                Easing::Linear,
                Repeat::Once,
            )],
            ..SceneObject::default()
        });
        let samples = s.sampled_at(50);
        assert_eq!(samples.len(), 1);
        // Base (1,2) + animation (10,20) = (11,22).
        assert!((samples[0].transform.position.0 - 11.0).abs() < 1e-4);
        assert!((samples[0].transform.position.1 - 22.0).abs() < 1e-4);
    }

    #[test]
    fn default_scene_has_no_lights() {
        let s = Scene::default();
        assert!(s.lights.is_empty());
        assert!(!s.has_lights());
    }

    #[test]
    fn push_light_appends_and_returns_index() {
        use crate::light::{Light, LightCommon, LightInstance};
        let mut s = Scene::default();
        let i0 = s.push_light(LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        }));
        let i1 = s.push_light(
            LightInstance::new(Light::Point {
                common: LightCommon::default(),
            })
            .with_position([5.0, 5.0, 0.0]),
        );
        assert_eq!(i0, 0);
        assert_eq!(i1, 1);
        assert!(s.has_lights());
        assert_eq!(s.lights.len(), 2);
        assert_eq!(s.lights[1].position, [5.0, 5.0, 0.0]);
    }

    #[test]
    fn lights_filter_selects_by_variant() {
        use crate::light::{Light, LightCommon, LightInstance, SpotParams};
        let mut s = Scene::default();
        s.push_light(LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        }));
        s.push_light(LightInstance::new(Light::Point {
            common: LightCommon::default(),
        }));
        s.push_light(LightInstance::new(Light::Point {
            common: LightCommon::default(),
        }));
        s.push_light(LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        }));
        assert_eq!(s.lights_filter(Light::is_directional).count(), 1);
        assert_eq!(s.lights_filter(Light::is_point).count(), 2);
        assert_eq!(s.lights_filter(Light::is_spot).count(), 1);
    }

    #[test]
    fn merge_concatenates_lights() {
        use crate::light::{Light, LightCommon, LightInstance};
        let mut a = Scene::default();
        a.push_light(LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        }));
        let mut b = Scene::default();
        b.push_light(
            LightInstance::new(Light::Point {
                common: LightCommon::default(),
            })
            .with_position([1.0, 0.0, 0.0]),
        );
        b.push_light(LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        }));
        a.merge(&b, 0, 0);
        assert_eq!(a.lights.len(), 3);
        // Position carried through verbatim from `b`.
        assert_eq!(a.lights[1].position, [1.0, 0.0, 0.0]);
    }
}
