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
    Decoded(Arc<oxideav_core::VideoFrame>),
    /// Filesystem path — resolved lazily by the renderer.
    Path(String),
    /// Raw bytes of an encoded image file (PNG/JPEG/etc).
    EncodedBytes(Arc<[u8]>),
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
}
