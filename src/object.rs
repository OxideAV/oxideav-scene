//! Scene objects — what's on the canvas, and where.

use std::sync::Arc;

use crate::animation::Animation;
use crate::duration::Lifetime;
use crate::id::ObjectId;

/// Pixel format alias; re-exports [`oxideav_core::PixelFormat`] so
/// callers don't need a direct core dependency just to build a
/// canvas.
pub use oxideav_core::PixelFormat;

/// Canvas — either pixel-based (NLE, compositor) or vector-coord
/// (PDF pages).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Canvas {
    /// Pixel raster. Used by the streaming compositor and the NLE
    /// timeline.
    Raster {
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    },
    /// Unit-agnostic vector canvas. PDF pages use this; the unit is
    /// whatever the producer declared. All scene coordinates live in
    /// this unit; rasterisation happens at export time.
    Vector {
        width: f32,
        height: f32,
        unit: LengthUnit,
    },
}

impl Canvas {
    /// Convenience for the common case: 8-bit 4:2:0 raster.
    pub const fn raster(width: u32, height: u32) -> Self {
        Canvas::Raster {
            width,
            height,
            pixel_format: PixelFormat::Yuv420P,
        }
    }

    /// Pixel dims for raster canvases, `None` for vector canvases.
    pub fn raster_size(&self) -> Option<(u32, u32)> {
        match self {
            Canvas::Raster { width, height, .. } => Some((*width, *height)),
            Canvas::Vector { .. } => None,
        }
    }
}

/// Length unit for vector canvases.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LengthUnit {
    /// PostScript / PDF point: 1/72 inch.
    #[default]
    Point,
    /// Millimetre.
    Millimetre,
    /// Inch.
    Inch,
    /// CSS pixel (96/in).
    CssPixel,
    /// Device pixel — what it is is device-dependent.
    DevicePixel,
}

/// One renderable element on a scene.
#[derive(Clone, Debug)]
pub struct SceneObject {
    pub id: ObjectId,
    pub kind: ObjectKind,
    pub transform: Transform,
    pub lifetime: Lifetime,
    pub animations: Vec<Animation>,
    pub z_order: i32,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub effects: Vec<Effect>,
    pub clip: Option<ClipRect>,
}

impl Default for SceneObject {
    fn default() -> Self {
        SceneObject {
            id: ObjectId::default(),
            kind: ObjectKind::Shape(Shape::rect(0.0, 0.0)),
            transform: Transform::identity(),
            lifetime: Lifetime::default(),
            animations: Vec::new(),
            z_order: 0,
            opacity: 1.0,
            blend_mode: BlendMode::default(),
            effects: Vec::new(),
            clip: None,
        }
    }
}

/// What a scene object IS.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ObjectKind {
    Image(ImageSource),
    Video(VideoSource),
    Text(TextRun),
    Shape(Shape),
    Group(Vec<ObjectId>),
    Live(LiveStreamHandle),
    /// Vector content — a self-contained
    /// [`oxideav_core::VectorFrame`]. Renders natively to vector
    /// outputs (PDF / SVG writers consume the `VectorFrame` as-is)
    /// and rasterises through `oxideav_raster::Renderer` for
    /// raster outputs (PNG / MP4 / RTMP); see
    /// [`crate::raster::rasterize_vector`] for the helper. The
    /// rasteriser also picks up `Group::cache_key` automatically
    /// when the same sub-tree is re-rendered.
    Vector(oxideav_core::VectorFrame),
}

impl ObjectKind {
    /// Object-local content extent for the kinds that carry one
    /// intrinsically.
    ///
    /// - [`ObjectKind::Vector`] — the underlying
    ///   [`oxideav_core::VectorFrame`]'s viewport `(width, height)`.
    /// - [`ObjectKind::Shape`] — delegates to [`Shape::content_size`].
    /// - [`ObjectKind::Live`] — the source's
    ///   [`hint_size`](LiveStreamHandle::hint_size), cast to `f32`
    ///   when present.
    /// - [`ObjectKind::Image`] — when the source is
    ///   [`ImageSource::Decoded`], the carried frame's pixel
    ///   dimensions decoded under the RGBA8-stride convention (see
    ///   [`ImageSource::natural_size`]). [`ImageSource::Path`] /
    ///   [`ImageSource::EncodedBytes`] return `None` — those need
    ///   a decoder the scene crate doesn't bind.
    /// - [`ObjectKind::Video`], [`ObjectKind::Text`],
    ///   [`ObjectKind::Group`] — return `None`. These kinds either
    ///   pull their extent from a frame the renderer fetches at
    ///   render time (video), from a shaping engine the scene crate
    ///   doesn't bind (text), or from their referenced children
    ///   resolved against a scene (group). Callers wanting a
    ///   geometry estimate for these kinds pass a fallback into
    ///   [`SceneObject::bbox`].
    pub fn content_size(&self) -> Option<(f32, f32)> {
        match self {
            ObjectKind::Vector(vf) => Some((vf.width, vf.height)),
            ObjectKind::Shape(s) => s.content_size(),
            ObjectKind::Live(h) => h.hint_size.map(|(w, h)| (w as f32, h as f32)),
            ObjectKind::Image(src) => src.natural_size().map(|(w, h)| (w as f32, h as f32)),
            ObjectKind::Video(_) | ObjectKind::Text(_) | ObjectKind::Group(_) => None,
        }
    }
}

impl SceneObject {
    /// Object-local content extent — sugar over
    /// [`ObjectKind::content_size`] for the object's own kind. See
    /// that method for which kinds report a size and which return
    /// `None`.
    pub fn content_size(&self) -> Option<(f32, f32)> {
        self.kind.content_size()
    }

    /// Axis-aligned bounding box of this object in canvas space.
    ///
    /// The intrinsic content extent is taken from
    /// [`SceneObject::content_size`] when available; otherwise
    /// `fallback` is used — pass the canvas size (or a per-object
    /// hint from the renderer) for kinds whose content size isn't
    /// known to the scene layer (raster images, video, text runs).
    /// The chosen extent is then run through
    /// [`Transform::bbox`](Transform::bbox) and finally intersected
    /// with [`SceneObject::clip`] if the object carries one.
    ///
    /// Clipping is conservative: the returned rectangle is the
    /// *intersection* of the transformed content AABB with the clip
    /// rect, expressed in canvas coordinates. The clip's coordinates
    /// are interpreted as already living in canvas space (matching
    /// the [`ClipRect`] doc-comment). When the intersection is empty
    /// the returned rect has zero width / height — the caller can
    /// detect culling by checking `rect.width == 0.0 ||
    /// rect.height == 0.0`.
    pub fn bbox(&self, fallback: (f32, f32)) -> oxideav_core::Rect {
        let (w, h) = self.content_size().unwrap_or(fallback);
        let bb = self.transform.bbox(w, h);
        match self.clip {
            None => bb,
            Some(clip) => intersect_rect(bb, clip),
        }
    }

    /// Find the *first* animation track on this object whose
    /// [`AnimatedProperty`](crate::animation::AnimatedProperty) matches
    /// `prop` and sample it at scene time `t`. Returns the raw
    /// [`KeyframeValue`](crate::animation::KeyframeValue) the track
    /// emits — no merging with the object's base
    /// [`Transform`] / [`opacity`](Self::opacity) is performed here.
    ///
    /// Returns `None` when the object carries no track for `prop` or
    /// when the matching track has no keyframes. Tracks are searched
    /// in insertion order; if two tracks animate the same property
    /// (currently allowed by
    /// [`Operation::Animate`](crate::ops::Operation::Animate)) only
    /// the first is consulted — the second is effectively shadowed
    /// until a [`CancelAnimation`](crate::ops::Operation::CancelAnimation)
    /// removes the leader.
    pub fn evaluate_property_at(
        &self,
        t: crate::duration::TimeStamp,
        prop: &crate::animation::AnimatedProperty,
    ) -> Option<crate::animation::KeyframeValue> {
        let anim = self.animations.iter().find(|a| &a.property == prop)?;
        anim.sample(t)
    }

    /// Compose the object's base [`Transform`] with any
    /// [`Position`](crate::animation::AnimatedProperty::Position) /
    /// [`Scale`](crate::animation::AnimatedProperty::Scale) /
    /// [`Rotation`](crate::animation::AnimatedProperty::Rotation) /
    /// [`Skew`](crate::animation::AnimatedProperty::Skew) /
    /// [`Anchor`](crate::animation::AnimatedProperty::Anchor)
    /// animation tracks evaluated at scene time `t`.
    ///
    /// Composition rule (per property):
    ///
    /// - `Position` (Vec2) — *added* to base `position`. Animations
    ///   are offsets from the base, matching the documented
    ///   `Operation::SetTransform` semantics ("animations on the
    ///   same object continue to add to this base").
    /// - `Scale` (Vec2) — *multiplied* with base `scale`. Matches the
    ///   convention used by After Effects / Lottie scale tracks.
    /// - `Rotation` (Scalar, radians) — *added* to base `rotation`.
    /// - `Skew` (Vec2, radians) — *added* to base `skew`.
    /// - `Anchor` (Vec2, normalised 0..=1) — *replaces* base `anchor`.
    ///   Anchors are pivot points, not deltas, so addition would be
    ///   meaningless; the animated value is used verbatim.
    ///
    /// Variant mismatches between the base field type and the track's
    /// [`KeyframeValue`] (e.g. an `Animation` on `Position` carrying a
    /// `Scalar`) are silently ignored — the base value passes
    /// through. Animation tracks targeting non-transform properties
    /// (`Opacity`, `Volume`, `EffectParam`, `Custom`) are likewise
    /// ignored by this method.
    pub fn effective_transform_at(&self, t: crate::duration::TimeStamp) -> Transform {
        use crate::animation::{AnimatedProperty as P, KeyframeValue as V};
        let mut out = self.transform;
        for prop in [P::Position, P::Scale, P::Rotation, P::Skew, P::Anchor] {
            let Some(v) = self.evaluate_property_at(t, &prop) else {
                continue;
            };
            match (prop, v) {
                (P::Position, V::Vec2(dx, dy)) => {
                    out.position = (out.position.0 + dx, out.position.1 + dy);
                }
                (P::Scale, V::Vec2(sx, sy)) => {
                    out.scale = (out.scale.0 * sx, out.scale.1 * sy);
                }
                (P::Rotation, V::Scalar(r)) => {
                    out.rotation += r;
                }
                (P::Skew, V::Vec2(kx, ky)) => {
                    out.skew = (out.skew.0 + kx, out.skew.1 + ky);
                }
                (P::Anchor, V::Vec2(ax, ay)) => {
                    out.anchor = (ax, ay);
                }
                _ => {} // variant mismatch — base value passes through.
            }
        }
        out
    }

    /// Compose the object's base [`opacity`](Self::opacity) with any
    /// [`Opacity`](crate::animation::AnimatedProperty::Opacity)
    /// animation track evaluated at scene time `t`.
    ///
    /// The animated value *multiplies* the base — a base of `0.5` and
    /// an animated `Scalar(0.5)` yields `0.25`. The result is clamped
    /// to `0.0..=1.0` so the caller can hand it straight to a
    /// compositor's alpha channel without re-clamping.
    ///
    /// Variant mismatches (a non-`Scalar` keyframe on an `Opacity`
    /// track) are ignored — the base value passes through clamped.
    pub fn effective_opacity_at(&self, t: crate::duration::TimeStamp) -> f32 {
        use crate::animation::{AnimatedProperty as P, KeyframeValue as V};
        let base = self.opacity;
        let factor = match self.evaluate_property_at(t, &P::Opacity) {
            Some(V::Scalar(v)) => v,
            _ => 1.0,
        };
        (base * factor).clamp(0.0, 1.0)
    }

    /// Evaluate every animation track on this object at scene time
    /// `t` and return a [`Sample`] carrying the resolved transform +
    /// opacity. The object's `kind`, `z_order`, `blend_mode`, `clip`
    /// and `id` are forwarded verbatim from `self`.
    ///
    /// This is the single-call entry point a renderer uses per object
    /// per frame: it hides the per-property dispatch and produces a
    /// pre-merged state that can be fed straight to the compositor.
    pub fn sample_at(&self, t: crate::duration::TimeStamp) -> Sample {
        Sample {
            id: self.id,
            z_order: self.z_order,
            transform: self.effective_transform_at(t),
            opacity: self.effective_opacity_at(t),
            blend_mode: self.blend_mode,
            clip: self.clip,
        }
    }
}

/// Per-object resolved state at a single scene time. Produced by
/// [`SceneObject::sample_at`] (and [`crate::Scene::sampled_at`]) so
/// renderers consume a flat, animation-merged view of each visible
/// object rather than threading [`Animation`](crate::animation::Animation)
/// evaluation through their own pipeline.
///
/// The forwarded fields (`id`, `z_order`, `blend_mode`, `clip`) come
/// from the source [`SceneObject`] unchanged; `transform` is the
/// composed base + animation-track result (see
/// [`SceneObject::effective_transform_at`]); `opacity` is the
/// composed + clamped value from
/// [`SceneObject::effective_opacity_at`]. The object's `kind` is not
/// inlined here — the renderer typically already holds the source
/// [`SceneObject`] for that and inlining `kind` would defeat the
/// "cheap to clone per frame" goal of this struct.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub id: ObjectId,
    pub z_order: i32,
    pub transform: Transform,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub clip: Option<ClipRect>,
}

/// Intersect the transformed-object AABB with a [`ClipRect`] given
/// in canvas space. Returns a [`Rect`] with non-negative extent;
/// extent is zero on both axes when the rectangles do not overlap.
fn intersect_rect(a: oxideav_core::Rect, clip: ClipRect) -> oxideav_core::Rect {
    let ax2 = a.x + a.width;
    let ay2 = a.y + a.height;
    let bx1 = clip.x;
    let by1 = clip.y;
    let bx2 = clip.x + clip.width;
    let by2 = clip.y + clip.height;
    let x1 = a.x.max(bx1);
    let y1 = a.y.max(by1);
    let x2 = ax2.min(bx2);
    let y2 = ay2.min(by2);
    if x2 <= x1 || y2 <= y1 {
        oxideav_core::Rect::new(x1, y1, 0.0, 0.0)
    } else {
        oxideav_core::Rect::new(x1, y1, x2 - x1, y2 - y1)
    }
}

/// Affine placement on the canvas. Applied in this order:
/// translate → anchor-relative rotate → scale → skew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: (f32, f32),
    pub scale: (f32, f32),
    /// Radians, counter-clockwise, around `anchor`.
    pub rotation: f32,
    /// Pivot point in normalised object-local coordinates (0..=1).
    /// `(0.5, 0.5)` is the object centre.
    pub anchor: (f32, f32),
    /// Shear in radians, per axis.
    pub skew: (f32, f32),
}

impl Transform {
    pub const fn identity() -> Self {
        Transform {
            position: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.5, 0.5),
            skew: (0.0, 0.0),
        }
    }

    /// Lower this high-level transform into a flat
    /// [`oxideav_core::Transform2D`] (the SVG / PDF `matrix(a,b,c,d,e,f)`
    /// form) for a content box of the given `(width, height)`.
    ///
    /// The struct's per-field semantics are realised in the documented
    /// order: a point in object-local space is first moved so the
    /// normalised [`anchor`](Self::anchor) sits at the origin, then
    /// rotated, scaled, and sheared about that anchor, and finally
    /// translated by [`position`](Self::position). Concretely the
    /// returned matrix `M` satisfies
    ///
    /// ```text
    /// M = T(position) · T(+pivot) · skew · scale · rotate · T(-pivot)
    /// ```
    ///
    /// where `pivot = (anchor.0 * width, anchor.1 * height)`. Applying
    /// `M` to a local point yields its canvas-space coordinate. The
    /// identity [`Transform`] over any content size lowers to
    /// [`Transform2D::identity`](oxideav_core::Transform2D::identity).
    ///
    /// `width` / `height` are the object's intrinsic content extent in
    /// canvas units — only the anchor pivot depends on them, so a
    /// zero-size content box still produces a well-formed (pivot-at-
    /// origin) matrix.
    pub fn to_matrix(&self, width: f32, height: f32) -> oxideav_core::Transform2D {
        use oxideav_core::Transform2D as M;

        let (px, py) = (self.anchor.0 * width, self.anchor.1 * height);

        // Built right-to-left so the leftmost factor is applied last:
        // start at the anchor-origin shift, then rotate, scale, skew,
        // re-apply the pivot, and finally translate into place.
        let mut m = M::translate(self.position.0, self.position.1);
        m = m.compose(&M::translate(px, py));
        // Skew: shear-X then shear-Y, matching Premiere's per-axis skew.
        if self.skew.0 != 0.0 {
            m = m.compose(&M::skew_x(self.skew.0));
        }
        if self.skew.1 != 0.0 {
            m = m.compose(&M::skew_y(self.skew.1));
        }
        m = m.compose(&M::scale(self.scale.0, self.scale.1));
        if self.rotation != 0.0 {
            m = m.compose(&M::rotate(self.rotation));
        }
        m = m.compose(&M::translate(-px, -py));
        m
    }

    /// Map an object-local point into canvas space under this
    /// transform, for a content box of `(width, height)`. Convenience
    /// over [`to_matrix`](Self::to_matrix) +
    /// [`Transform2D::apply`](oxideav_core::Transform2D::apply).
    pub fn apply_to_point(
        &self,
        width: f32,
        height: f32,
        point: oxideav_core::Point,
    ) -> oxideav_core::Point {
        self.to_matrix(width, height).apply(point)
    }

    /// Axis-aligned bounding box, in canvas space, of a
    /// `(width, height)` content box placed at the local origin
    /// `(0, 0)..(width, height)` and run through this transform.
    ///
    /// Computed by mapping the box's four corners and taking the min /
    /// max of the results, so it is tight for translate / scale / skew
    /// and a correct (rotation-aware) enclosing box for rotations —
    /// the AABB grows to contain a rotated rectangle rather than
    /// rotating with it. The returned [`oxideav_core::Rect`] always has
    /// non-negative `width` / `height`.
    pub fn bbox(&self, width: f32, height: f32) -> oxideav_core::Rect {
        use oxideav_core::Point;

        let m = self.to_matrix(width, height);
        let corners = [
            m.apply(Point::new(0.0, 0.0)),
            m.apply(Point::new(width, 0.0)),
            m.apply(Point::new(width, height)),
            m.apply(Point::new(0.0, height)),
        ];
        let mut min_x = corners[0].x;
        let mut min_y = corners[0].y;
        let mut max_x = corners[0].x;
        let mut max_y = corners[0].y;
        for p in &corners[1..] {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
        oxideav_core::Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Transform::identity()
    }
}

/// Compositing blend — painter's algorithm default is `Normal`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Add,
    /// Subtract destination from source.
    Subtract,
    /// Source replaces destination even in transparent regions —
    /// useful for mask objects.
    Copy,
}

/// Filter applied to the object's raster output before compositing.
/// The parameter map is opaque here; per-effect implementations in
/// sibling crates interpret it.
#[derive(Clone, Debug)]
pub struct Effect {
    pub name: String,
    pub params: Vec<(String, f32)>,
}

/// Axis-aligned clipping rectangle in canvas coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Bitmap source. Either an owned frame, a shared frame handle, or
/// a path that the renderer resolves on first use.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ImageSource {
    /// Fully-decoded frame, `Arc`-shared so cloning is cheap.
    ///
    /// The renderer treats the carried [`oxideav_core::VideoFrame`] as a
    /// single straight-alpha RGBA8 plane: `planes[0].stride` carries the
    /// pixel width as `stride / 4`, and `planes[0].data.len() / stride`
    /// carries the pixel height. This matches the convention that the
    /// `oxideav-raster` rasteriser emits and reads at the
    /// `Node::Image` sampling boundary, so a frame produced by
    /// `oxideav_raster::Renderer::render` round-trips through
    /// `Decoded(_)` without an intermediate conversion.
    Decoded(Arc<oxideav_core::VideoFrame>),
    /// Filesystem path — resolved lazily by the renderer.
    Path(String),
    /// Raw bytes of an encoded image file (PNG/JPEG/etc).
    EncodedBytes(Arc<[u8]>),
}

impl ImageSource {
    /// Natural `(width, height)` in pixels when known to the scene
    /// crate without invoking a decoder.
    ///
    /// - [`ImageSource::Decoded`] reports the carried frame's first
    ///   plane decoded under the RGBA8-stride convention documented on
    ///   the variant: `width = stride / 4`,
    ///   `height = data.len() / stride`. Frames with a missing /
    ///   degenerate first plane (`stride < 4`, or `stride` not divisible
    ///   into `data.len()`) report `None`.
    /// - [`ImageSource::Path`] and [`ImageSource::EncodedBytes`] always
    ///   return `None` — extracting the natural dimensions would
    ///   require a decoder the scene crate doesn't bind. Callers that
    ///   need the size pre-decode an upstream frame and pass it via
    ///   `Decoded`.
    pub fn natural_size(&self) -> Option<(u32, u32)> {
        match self {
            ImageSource::Decoded(frame) => decoded_rgba_size(frame),
            ImageSource::Path(_) | ImageSource::EncodedBytes(_) => None,
        }
    }
}

/// Decode the canonical RGBA8 plane convention from a
/// [`oxideav_core::VideoFrame`]: width is `stride/4`, height is
/// `data.len()/stride`. Returns `None` for frames that don't follow
/// the convention (no planes, `stride < 4`, `stride` not divisible
/// into `data.len()`).
pub(crate) fn decoded_rgba_size(frame: &oxideav_core::VideoFrame) -> Option<(u32, u32)> {
    let plane = frame.planes.first()?;
    if plane.stride < 4 {
        return None;
    }
    let width = plane.stride / 4;
    if width == 0 {
        return None;
    }
    if plane.data.len() % plane.stride != 0 {
        return None;
    }
    let height = plane.data.len() / plane.stride;
    if height == 0 {
        return None;
    }
    Some((width as u32, height as u32))
}

/// Video source. Resolves packets via the container layer on
/// demand; the scene renderer advances it to the requested PTS.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum VideoSource {
    Path(String),
    EncodedBytes(Arc<[u8]>),
}

/// Styled text run. Font resolution + shaping land in a separate
/// crate; this type only carries what the model needs to preserve
/// (the string itself + structural + appearance metadata).
#[derive(Clone, Debug, Default)]
pub struct TextRun {
    pub text: String,
    pub font_family: String,
    pub font_weight: u16,
    pub font_size: f32,
    /// `0xRRGGBBAA`.
    pub color: u32,
    /// Optional explicit glyph-advance vector (PDF-style). If
    /// `None`, the rasteriser shapes on the fly.
    pub advances: Option<Vec<f32>>,
    pub italic: bool,
    pub underline: bool,
}

/// Vector shape primitive.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Shape {
    Rect {
        width: f32,
        height: f32,
        fill: u32,
        stroke: Option<Stroke>,
        corner_radius: f32,
    },
    Polygon {
        points: Vec<(f32, f32)>,
        fill: u32,
        stroke: Option<Stroke>,
    },
    Path {
        /// SVG path data ("M10,10 L20,20 …").
        data: String,
        fill: u32,
        stroke: Option<Stroke>,
    },
}

impl Shape {
    /// Zero-size placeholder rect with no fill. Used by
    /// `SceneObject::default`.
    pub const fn rect(width: f32, height: f32) -> Self {
        Shape::Rect {
            width,
            height,
            fill: 0,
            stroke: None,
            corner_radius: 0.0,
        }
    }

    /// Object-local content extent — the `(width, height)` of the
    /// minimal axis-aligned box that contains the shape's geometry
    /// in its own coordinate system (before any [`Transform`] is
    /// applied).
    ///
    /// - [`Shape::Rect`] reports its declared `(width, height)`
    ///   verbatim. A rounded rect with `corner_radius > 0` still has
    ///   the same outer bound; the rounding only carves area away
    ///   *inside* the box.
    /// - [`Shape::Polygon`] reports the bounding box of its `points`
    ///   list. An empty polygon reports `(0.0, 0.0)`.
    /// - [`Shape::Path`] is parsed by [`crate::svg_path::parse_bbox`]
    ///   and reports the AABB of every anchor / control point. This is
    ///   the convex-hull-of-control-points superset of the painted
    ///   curve (an exact tight bound would walk the derivative roots);
    ///   it is what scene-layer layout queries actually want. Returns
    ///   `None` for empty / unparseable data.
    ///
    /// Stroke half-widths are NOT included; the bounds reflect the
    /// filled geometry only. A rasteriser that needs the stroked
    /// silhouette must inflate the result by `stroke.width / 2`.
    pub fn content_size(&self) -> Option<(f32, f32)> {
        match self {
            Shape::Rect { width, height, .. } => Some((*width, *height)),
            Shape::Polygon { points, .. } => {
                if points.is_empty() {
                    return Some((0.0, 0.0));
                }
                let (mut min_x, mut min_y) = points[0];
                let (mut max_x, mut max_y) = (min_x, min_y);
                for &(x, y) in &points[1..] {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
                Some(((max_x - min_x).max(0.0), (max_y - min_y).max(0.0)))
            }
            Shape::Path { data, .. } => {
                crate::svg_path::parse_bbox(data).map(|(min_x, min_y, max_x, max_y)| {
                    ((max_x - min_x).max(0.0), (max_y - min_y).max(0.0))
                })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub color: u32,
    pub width: f32,
}

/// Opaque handle to a live input feed. The renderer polls it for
/// the most recent frame at render time.
#[derive(Clone, Debug)]
pub struct LiveStreamHandle {
    /// Implementation-defined URI — `rtmp://…`, `file://named-pipe`,
    /// etc. The streaming compositor resolves this against a
    /// pluggable `LiveSource` registry (pending crate).
    pub uri: String,
    /// Optional hint for the expected frame size. The renderer will
    /// fall back to the actual frame size if it differs.
    pub hint_size: Option<(u32, u32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_canvas_size() {
        let c = Canvas::raster(640, 480);
        assert_eq!(c.raster_size(), Some((640, 480)));
    }

    #[test]
    fn vector_canvas_no_raster_size() {
        let c = Canvas::Vector {
            width: 595.0,
            height: 842.0,
            unit: LengthUnit::Point,
        };
        assert!(c.raster_size().is_none());
    }

    #[test]
    fn transform_identity_roundtrip() {
        let t = Transform::identity();
        assert_eq!(t.position, (0.0, 0.0));
        assert_eq!(t.scale, (1.0, 1.0));
        assert_eq!(t.anchor, (0.5, 0.5));
    }

    #[test]
    fn scene_object_default_is_neutral() {
        let o = SceneObject::default();
        assert_eq!(o.opacity, 1.0);
        assert_eq!(o.blend_mode, BlendMode::Normal);
        assert!(o.animations.is_empty());
    }

    #[test]
    fn identity_transform_lowers_to_identity_matrix() {
        let m = Transform::identity().to_matrix(100.0, 50.0);
        assert!(m.is_identity());
    }

    #[test]
    fn translate_only_offsets_points() {
        let t = Transform {
            position: (10.0, -5.0),
            ..Transform::identity()
        };
        // Pure translation is anchor-independent.
        let p = t.apply_to_point(40.0, 40.0, oxideav_core::Point::new(3.0, 7.0));
        assert!((p.x - 13.0).abs() < 1e-5);
        assert!((p.y - 2.0).abs() < 1e-5);
    }

    #[test]
    fn scale_pivots_about_anchor_centre() {
        // Anchor at centre of a 20x20 box → pivot (10,10). 2x scale
        // keeps the pivot fixed and pushes corners out symmetrically.
        let t = Transform {
            scale: (2.0, 2.0),
            ..Transform::identity()
        };
        let centre = t.apply_to_point(20.0, 20.0, oxideav_core::Point::new(10.0, 10.0));
        assert!((centre.x - 10.0).abs() < 1e-5);
        assert!((centre.y - 10.0).abs() < 1e-5);
        let bb = t.bbox(20.0, 20.0);
        // 20x20 scaled 2x about centre → 40x40 centred on (10,10).
        assert!((bb.width - 40.0).abs() < 1e-4);
        assert!((bb.height - 40.0).abs() < 1e-4);
        assert!((bb.x - (-10.0)).abs() < 1e-4);
        assert!((bb.y - (-10.0)).abs() < 1e-4);
    }

    #[test]
    fn quarter_turn_bbox_swaps_extent() {
        // 90° rotation of a 40x10 box about its centre → AABB 10x40.
        let t = Transform {
            rotation: std::f32::consts::FRAC_PI_2,
            ..Transform::identity()
        };
        let bb = t.bbox(40.0, 10.0);
        assert!((bb.width - 10.0).abs() < 1e-3);
        assert!((bb.height - 40.0).abs() < 1e-3);
    }

    #[test]
    fn bbox_extent_is_never_negative() {
        let t = Transform {
            scale: (-3.0, 0.5),
            rotation: 1.1,
            skew: (0.3, -0.2),
            position: (12.0, -4.0),
            anchor: (0.25, 0.75),
        };
        let bb = t.bbox(30.0, 18.0);
        assert!(bb.width >= 0.0);
        assert!(bb.height >= 0.0);
    }

    #[test]
    fn shape_rect_reports_its_own_extent() {
        let s = Shape::Rect {
            width: 80.0,
            height: 30.0,
            fill: 0,
            stroke: None,
            corner_radius: 4.0,
        };
        assert_eq!(s.content_size(), Some((80.0, 30.0)));
    }

    #[test]
    fn shape_polygon_reports_aabb_of_points() {
        let s = Shape::Polygon {
            points: vec![(-3.0, 5.0), (10.0, -2.0), (7.0, 12.0)],
            fill: 0,
            stroke: None,
        };
        // x ∈ [-3, 10] → width 13. y ∈ [-2, 12] → height 14.
        assert_eq!(s.content_size(), Some((13.0, 14.0)));
    }

    #[test]
    fn empty_polygon_has_zero_extent() {
        let s = Shape::Polygon {
            points: Vec::new(),
            fill: 0,
            stroke: None,
        };
        assert_eq!(s.content_size(), Some((0.0, 0.0)));
    }

    #[test]
    fn shape_path_extent_is_parsed_aabb() {
        let s = Shape::Path {
            data: "M10,10 L20,20".to_string(),
            fill: 0,
            stroke: None,
        };
        // AABB of (10,10)..(20,20) → 10x10 extent.
        assert_eq!(s.content_size(), Some((10.0, 10.0)));
    }

    #[test]
    fn shape_path_unparseable_returns_none() {
        let s = Shape::Path {
            data: "totally-not-a-path".to_string(),
            fill: 0,
            stroke: None,
        };
        assert!(s.content_size().is_none());
    }

    #[test]
    fn shape_path_arc_bbox_covers_endpoints() {
        // Arcs now parse to `PathCommand::ArcTo`; the conservative
        // bbox expands each arc endpoint by max(rx, ry) on both axes,
        // so a 5-radius quarter-arc from (0,0) to (10,10) is bounded
        // by at least the (-5, -5)..=(15, 15) box.
        let s = Shape::Path {
            data: "M0,0 A 5 5 0 0 0 10 10".to_string(),
            fill: 0,
            stroke: None,
        };
        let (w, h) = s.content_size().unwrap();
        // (max - min) is the content_size return; max_x - min_x ≥ 15 - (-5) = 20.
        assert!(w >= 20.0, "content width too tight: {w}");
        assert!(h >= 20.0, "content height too tight: {h}");
    }

    #[test]
    fn live_kind_uses_hint_size_when_present() {
        let live = ObjectKind::Live(LiveStreamHandle {
            uri: "rtmp://x".into(),
            hint_size: Some((1280, 720)),
        });
        assert_eq!(live.content_size(), Some((1280.0, 720.0)));
        let live_blank = ObjectKind::Live(LiveStreamHandle {
            uri: "rtmp://x".into(),
            hint_size: None,
        });
        assert!(live_blank.content_size().is_none());
    }

    #[test]
    fn vector_kind_pulls_extent_from_frame_viewport() {
        let vf = oxideav_core::VectorFrame::new(640.0, 480.0);
        let k = ObjectKind::Vector(vf);
        assert_eq!(k.content_size(), Some((640.0, 480.0)));
    }

    #[test]
    fn text_and_group_have_no_intrinsic_extent() {
        assert!(ObjectKind::Text(TextRun::default())
            .content_size()
            .is_none());
        assert!(ObjectKind::Group(Vec::new()).content_size().is_none());
    }

    #[test]
    fn image_kind_picks_up_decoded_natural_size() {
        use oxideav_core::{VideoFrame, VideoPlane};
        use std::sync::Arc;
        // 6 px wide × 3 px tall RGBA8 frame: stride = 24 = 6 * 4.
        let frame = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 24,
                data: vec![0u8; 24 * 3],
            }],
        };
        let kind = ObjectKind::Image(ImageSource::Decoded(Arc::new(frame)));
        assert_eq!(kind.content_size(), Some((6.0, 3.0)));
    }

    #[test]
    fn image_kind_with_encoded_source_has_no_intrinsic_extent() {
        let path_kind = ObjectKind::Image(ImageSource::Path("x.png".into()));
        assert!(path_kind.content_size().is_none());
        let bytes_kind = ObjectKind::Image(ImageSource::EncodedBytes(
            vec![0x89, 0x50, 0x4e, 0x47].into(),
        ));
        assert!(bytes_kind.content_size().is_none());
    }

    #[test]
    fn scene_object_bbox_uses_intrinsic_extent() {
        let obj = SceneObject {
            kind: ObjectKind::Shape(Shape::Rect {
                width: 40.0,
                height: 20.0,
                fill: 0,
                stroke: None,
                corner_radius: 0.0,
            }),
            transform: Transform {
                position: (5.0, 7.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        };
        // Fallback is ignored: the shape supplies its own (40, 20).
        let bb = obj.bbox((1000.0, 1000.0));
        assert!((bb.x - 5.0).abs() < 1e-4);
        assert!((bb.y - 7.0).abs() < 1e-4);
        assert!((bb.width - 40.0).abs() < 1e-4);
        assert!((bb.height - 20.0).abs() < 1e-4);
    }

    #[test]
    fn scene_object_bbox_falls_back_for_extentless_kinds() {
        let obj = SceneObject {
            kind: ObjectKind::Text(TextRun::default()),
            transform: Transform {
                position: (10.0, 20.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        };
        let bb = obj.bbox((100.0, 50.0));
        assert!((bb.x - 10.0).abs() < 1e-4);
        assert!((bb.y - 20.0).abs() < 1e-4);
        assert!((bb.width - 100.0).abs() < 1e-4);
        assert!((bb.height - 50.0).abs() < 1e-4);
    }

    #[test]
    fn scene_object_bbox_clips_to_clip_rect() {
        let obj = SceneObject {
            kind: ObjectKind::Shape(Shape::Rect {
                width: 100.0,
                height: 100.0,
                fill: 0,
                stroke: None,
                corner_radius: 0.0,
            }),
            transform: Transform::identity(),
            clip: Some(ClipRect {
                x: 20.0,
                y: 30.0,
                width: 50.0,
                height: 40.0,
            }),
            ..SceneObject::default()
        };
        let bb = obj.bbox((0.0, 0.0));
        assert!((bb.x - 20.0).abs() < 1e-4);
        assert!((bb.y - 30.0).abs() < 1e-4);
        assert!((bb.width - 50.0).abs() < 1e-4);
        assert!((bb.height - 40.0).abs() < 1e-4);
    }

    #[test]
    fn scene_object_bbox_clip_with_no_overlap_collapses_to_zero() {
        let obj = SceneObject {
            kind: ObjectKind::Shape(Shape::Rect {
                width: 10.0,
                height: 10.0,
                fill: 0,
                stroke: None,
                corner_radius: 0.0,
            }),
            transform: Transform::identity(),
            clip: Some(ClipRect {
                x: 500.0,
                y: 500.0,
                width: 50.0,
                height: 50.0,
            }),
            ..SceneObject::default()
        };
        let bb = obj.bbox((0.0, 0.0));
        assert!(bb.width <= 0.0 || bb.height <= 0.0);
    }

    // ----- effective_transform_at / effective_opacity_at / sample_at -----

    use crate::animation::{
        AnimatedProperty as P, Animation, Easing, Keyframe, KeyframeValue as V, Repeat,
    };

    fn scalar_anim(prop: P, kf: &[(crate::duration::TimeStamp, f32)]) -> Animation {
        Animation::new(
            prop,
            kf.iter()
                .map(|(t, v)| Keyframe {
                    time: *t,
                    value: V::Scalar(*v),
                    easing: None,
                })
                .collect(),
            Easing::Linear,
            Repeat::Once,
        )
    }

    fn vec2_anim(prop: P, kf: &[(crate::duration::TimeStamp, (f32, f32))]) -> Animation {
        Animation::new(
            prop,
            kf.iter()
                .map(|(t, (x, y))| Keyframe {
                    time: *t,
                    value: V::Vec2(*x, *y),
                    easing: None,
                })
                .collect(),
            Easing::Linear,
            Repeat::Once,
        )
    }

    #[test]
    fn evaluate_property_at_returns_none_without_track() {
        let obj = SceneObject::default();
        assert!(obj.evaluate_property_at(0, &P::Opacity).is_none());
    }

    #[test]
    fn evaluate_property_at_returns_raw_keyframe_value() {
        let obj = SceneObject {
            animations: vec![scalar_anim(P::Opacity, &[(0, 0.0), (100, 1.0)])],
            ..SceneObject::default()
        };
        let v = obj.evaluate_property_at(50, &P::Opacity).unwrap();
        match v {
            V::Scalar(s) => assert!((s - 0.5).abs() < 1e-4),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn effective_transform_with_no_animation_is_base() {
        let obj = SceneObject {
            transform: Transform {
                position: (10.0, 20.0),
                scale: (2.0, 3.0),
                rotation: 0.5,
                anchor: (0.25, 0.75),
                skew: (0.1, 0.2),
            },
            ..SceneObject::default()
        };
        assert_eq!(obj.effective_transform_at(123), obj.transform);
    }

    #[test]
    fn position_track_adds_to_base() {
        let obj = SceneObject {
            transform: Transform {
                position: (5.0, 7.0),
                ..Transform::identity()
            },
            animations: vec![vec2_anim(
                P::Position,
                &[(0, (10.0, 20.0)), (100, (10.0, 20.0))],
            )],
            ..SceneObject::default()
        };
        let t = obj.effective_transform_at(50);
        assert!((t.position.0 - 15.0).abs() < 1e-4);
        assert!((t.position.1 - 27.0).abs() < 1e-4);
    }

    #[test]
    fn scale_track_multiplies_with_base() {
        let obj = SceneObject {
            transform: Transform {
                scale: (2.0, 3.0),
                ..Transform::identity()
            },
            animations: vec![vec2_anim(P::Scale, &[(0, (1.5, 2.0)), (100, (1.5, 2.0))])],
            ..SceneObject::default()
        };
        let t = obj.effective_transform_at(50);
        assert!((t.scale.0 - 3.0).abs() < 1e-4);
        assert!((t.scale.1 - 6.0).abs() < 1e-4);
    }

    #[test]
    fn rotation_track_adds_to_base() {
        let obj = SceneObject {
            transform: Transform {
                rotation: 1.0,
                ..Transform::identity()
            },
            animations: vec![scalar_anim(P::Rotation, &[(0, 0.5), (100, 0.5)])],
            ..SceneObject::default()
        };
        assert!((obj.effective_transform_at(50).rotation - 1.5).abs() < 1e-4);
    }

    #[test]
    fn skew_track_adds_to_base() {
        let obj = SceneObject {
            transform: Transform {
                skew: (0.2, 0.3),
                ..Transform::identity()
            },
            animations: vec![vec2_anim(P::Skew, &[(0, (0.1, -0.1)), (100, (0.1, -0.1))])],
            ..SceneObject::default()
        };
        let t = obj.effective_transform_at(50);
        assert!((t.skew.0 - 0.3).abs() < 1e-4);
        assert!((t.skew.1 - 0.2).abs() < 1e-4);
    }

    #[test]
    fn anchor_track_replaces_base() {
        let obj = SceneObject {
            transform: Transform {
                anchor: (0.5, 0.5),
                ..Transform::identity()
            },
            animations: vec![vec2_anim(
                P::Anchor,
                &[(0, (0.25, 0.75)), (100, (0.25, 0.75))],
            )],
            ..SceneObject::default()
        };
        let t = obj.effective_transform_at(50);
        assert!((t.anchor.0 - 0.25).abs() < 1e-4);
        assert!((t.anchor.1 - 0.75).abs() < 1e-4);
    }

    #[test]
    fn variant_mismatch_on_transform_track_falls_through() {
        // Position expects Vec2; feeding it a Scalar is a no-op.
        let obj = SceneObject {
            transform: Transform {
                position: (3.0, 4.0),
                ..Transform::identity()
            },
            animations: vec![scalar_anim(P::Position, &[(0, 99.0), (100, 99.0)])],
            ..SceneObject::default()
        };
        let t = obj.effective_transform_at(50);
        assert!((t.position.0 - 3.0).abs() < 1e-4);
        assert!((t.position.1 - 4.0).abs() < 1e-4);
    }

    #[test]
    fn effective_opacity_no_track_is_base() {
        let obj = SceneObject {
            opacity: 0.7,
            ..SceneObject::default()
        };
        assert!((obj.effective_opacity_at(0) - 0.7).abs() < 1e-4);
    }

    #[test]
    fn effective_opacity_multiplies_and_clamps() {
        let obj = SceneObject {
            opacity: 0.8,
            animations: vec![scalar_anim(P::Opacity, &[(0, 0.5), (100, 0.5)])],
            ..SceneObject::default()
        };
        // 0.8 * 0.5 = 0.4
        assert!((obj.effective_opacity_at(50) - 0.4).abs() < 1e-4);
    }

    #[test]
    fn effective_opacity_clamps_to_unit_range() {
        // Base 1.0 * animated 2.0 would be 2.0; should clamp to 1.0.
        let obj = SceneObject {
            opacity: 1.0,
            animations: vec![scalar_anim(P::Opacity, &[(0, 2.0), (100, 2.0)])],
            ..SceneObject::default()
        };
        assert!((obj.effective_opacity_at(50) - 1.0).abs() < 1e-4);

        // Negative animated value would yield negative; should clamp to 0.0.
        let obj = SceneObject {
            opacity: 0.5,
            animations: vec![scalar_anim(P::Opacity, &[(0, -1.0), (100, -1.0)])],
            ..SceneObject::default()
        };
        assert!(obj.effective_opacity_at(50).abs() < 1e-4);
    }

    #[test]
    fn sample_at_forwards_compositor_fields() {
        let obj = SceneObject {
            id: ObjectId::new(42),
            opacity: 0.5,
            z_order: 7,
            blend_mode: BlendMode::Screen,
            clip: Some(ClipRect {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            }),
            transform: Transform {
                position: (10.0, 20.0),
                ..Transform::identity()
            },
            animations: vec![scalar_anim(P::Opacity, &[(0, 0.5), (100, 0.5)])],
            ..SceneObject::default()
        };
        let s = obj.sample_at(50);
        assert_eq!(s.id, ObjectId::new(42));
        assert_eq!(s.z_order, 7);
        assert_eq!(s.blend_mode, BlendMode::Screen);
        assert!(s.clip.is_some());
        assert!((s.opacity - 0.25).abs() < 1e-4); // 0.5 * 0.5
        assert!((s.transform.position.0 - 10.0).abs() < 1e-4);
    }

    #[test]
    fn multiple_transform_tracks_compose_independently() {
        let obj = SceneObject {
            transform: Transform {
                position: (1.0, 1.0),
                scale: (1.0, 1.0),
                rotation: 0.1,
                ..Transform::identity()
            },
            animations: vec![
                vec2_anim(P::Position, &[(0, (4.0, 5.0)), (100, (4.0, 5.0))]),
                scalar_anim(P::Rotation, &[(0, 0.4), (100, 0.4)]),
                vec2_anim(P::Scale, &[(0, (3.0, 4.0)), (100, (3.0, 4.0))]),
            ],
            ..SceneObject::default()
        };
        let t = obj.effective_transform_at(50);
        assert!((t.position.0 - 5.0).abs() < 1e-4); // 1+4
        assert!((t.position.1 - 6.0).abs() < 1e-4); // 1+5
        assert!((t.scale.0 - 3.0).abs() < 1e-4); // 1*3
        assert!((t.scale.1 - 4.0).abs() < 1e-4); // 1*4
        assert!((t.rotation - 0.5).abs() < 1e-4); // 0.1+0.4
    }
}
