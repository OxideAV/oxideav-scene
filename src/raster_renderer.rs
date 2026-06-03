//! A concrete [`SceneRenderer`] that composites the *vector* slice of a
//! scene — backgrounds, [`Shape`]s, and [`ObjectKind::Vector`] objects —
//! into a straight-alpha RGBA8 [`oxideav_core::VideoFrame`].
//!
//! This is the second concrete piece of real rendering after
//! [`crate::text::TextRenderer`]: where `TextRenderer` rasterises a
//! single [`TextRun`](crate::TextRun), [`RasterRenderer`] drives the full
//! [`SceneRenderer`] loop and walks [`Scene::sampled_at`] in paint order,
//! lowering each object's animation-merged [`Transform`] +
//! [`opacity`](crate::SceneObject::opacity) + clip into an
//! [`oxideav_core::VectorFrame`] which is then rasterised through
//! [`oxideav_raster::Renderer`].
//!
//! # Scope
//!
//! `RasterRenderer` handles the object kinds the scene crate can render
//! with no external resources:
//!
//! * [`Background`] — `Solid`, `Transparent`, two-colour `LinearGradient`,
//!   and the multi-stop [`Background::Gradient`] (linear + radial).
//! * [`ObjectKind::Shape`] — `Rect` (with corner radius), `Polygon`, and
//!   `Path` (SVG path data parsed via [`crate::svg_path::parse_path`],
//!   including elliptical arcs `A` / `a` which round-trip through
//!   [`oxideav_core::PathCommand::ArcTo`] and the raster pipeline's
//!   arc-to-cubic flattener; unparseable data is skipped without
//!   erroring the frame).
//! * [`ObjectKind::Vector`] — the carried [`oxideav_core::VectorFrame`]'s
//!   root group is inlined under the object's transform.
//! * [`ObjectKind::Group`] — child object ids are resolved against the
//!   scene and inlined under the group's own `Transform` / `opacity` /
//!   `clip` (composed multiplicatively over each child's own
//!   sampled state). Cycles in the child graph are broken at the
//!   second visit (each id rendered at most once per group expansion);
//!   missing ids are silently dropped.
//! * [`ObjectKind::Image`] with [`crate::ImageSource::Decoded`] — the
//!   carried [`oxideav_core::VideoFrame`] is wrapped in a
//!   [`oxideav_core::Node::Image`] under an
//!   [`oxideav_core::ImageRef`] whose `bounds` rectangle is the
//!   frame's natural `(width, height)` decoded under the RGBA8-stride
//!   convention (see [`crate::ImageSource::natural_size`]). The
//!   object's animation-merged [`Transform`] / opacity / clip wrap
//!   it as for every other kind, so a 16×16 source frame placed at
//!   `position = (5, 5)` paints into the canvas pixels `[5..21, 5..21)`
//!   (top-left anchor). The downstream
//!   [`oxideav_raster::Renderer`] samples the carried frame through
//!   its configured [`oxideav_raster::ImageFilter`] (bilinear by
//!   default; switch via [`oxideav_raster::Renderer::image_filter`]).
//! * [`ObjectKind::Video`] with [`crate::VideoSource::DecodedFrames`]
//!   — the carried frame sequence is sampled at the current scene
//!   time `t` via [`crate::VideoSource::frame_at`], which picks the
//!   frame whose presentation interval contains `t` (using each
//!   object's [`crate::Lifetime`] `start` as the sequence's `t = 0`).
//!   The chosen frame is wrapped in the same [`oxideav_core::ImageRef`]
//!   shape `Image(Decoded)` uses, so it composites under the object's
//!   animation-merged [`Transform`] / opacity / clip in the same
//!   paint-order pass as backgrounds, shapes, vector frames, images,
//!   and groups. Sequence-ending samples hold on the final frame
//!   until the object's lifetime expires — finite NLE clips freeze on
//!   their tail rather than flash black.
//!
//! The kinds that still need a decoder or a font face are **skipped**
//! (they contribute nothing to the frame, but never error):
//!
//! * [`ObjectKind::Image`] with [`crate::ImageSource::Path`] or
//!   [`crate::ImageSource::EncodedBytes`] — both still need a decoder
//!   binding the scene crate doesn't carry. Pre-decode upstream and
//!   feed the [`oxideav_core::VideoFrame`] back in via
//!   [`crate::ImageSource::Decoded`] until a decoder-aware renderer
//!   lands.
//! * [`ObjectKind::Video`] with [`crate::VideoSource::Path`] or
//!   [`crate::VideoSource::EncodedBytes`] — same decoder-bound shape
//!   as the encoded `Image` arms. Pre-decode upstream and feed back
//!   via [`crate::VideoSource::DecodedFrames`] until a decoder-aware
//!   renderer lands.
//! * [`ObjectKind::Live`] — needs a live source the scene crate
//!   doesn't bind. A future renderer composites the pulled frame via
//!   [`crate::adapt::adapt_frame_to_canvas`].
//! * [`ObjectKind::Text`] — needs a [`oxideav_scribe::Face`]; render it
//!   with [`crate::text::TextRenderer`] and composite the result, or wait
//!   for a font-registry-aware renderer.
//!
//! # Coordinate system
//!
//! A [`Canvas::Raster`] is addressed in pixels, so scene-space units map
//! 1:1 to raster pixels — the built [`oxideav_core::VectorFrame`] carries
//! no `view_box` and its root sits in pixel space. A [`Canvas::Vector`]
//! canvas is rejected with [`Error::unsupported`]: vector canvases are
//! for paged / PDF export, which consumes the `VectorFrame` directly
//! rather than rasterising.

use oxideav_core::{
    Error, FillRule, Group, ImageRef, LinearGradient, Node, Paint as CorePaint, Path, PathNode,
    Point, RadialGradient, Rect, Result, Rgba, Stroke as CoreStroke, TimeBase, Transform2D,
    VectorFrame, VideoFrame,
};

use crate::object::decoded_rgba_size;
use oxideav_raster::Renderer;

use std::collections::HashSet;

use crate::duration::TimeStamp;
use crate::id::ObjectId;
use crate::object::{
    ClipRect, ImageSource, ObjectKind, SceneObject, Shape, Stroke, Transform, VideoSource,
};
use crate::paint::{Gradient, Stop};
use crate::render::{RenderedFrame, SceneRenderer};
use crate::scene::{Background, Scene};
use crate::svg_path;

/// Concrete renderer for the vector slice of a scene. See the module
/// docs for the supported object kinds and the skip list.
///
/// Holds a reusable [`oxideav_raster::Renderer`] so the per-glyph /
/// per-group bitmap cache survives across `render_at` calls.
#[derive(Debug)]
pub struct RasterRenderer {
    renderer: Renderer,
}

impl Default for RasterRenderer {
    fn default() -> Self {
        Self {
            // Size is reset on every render_at to the scene's canvas.
            renderer: Renderer::new(1, 1),
        }
    }
}

impl RasterRenderer {
    /// Build a renderer with a fresh bitmap cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the [`VectorFrame`] for `scene` at time `t`. Exposed for
    /// callers (and tests) that want the intermediate vector tree
    /// without rasterising — e.g. to hand it to a vector writer.
    ///
    /// Returns `Err(Error::Unsupported)` for a vector (non-raster)
    /// canvas; see the module docs.
    pub fn build_frame(&self, scene: &Scene, t: TimeStamp) -> Result<VectorFrame> {
        let (w, h) = scene.canvas.raster_size().ok_or_else(|| {
            Error::unsupported(
                "oxideav-scene: RasterRenderer needs a Canvas::Raster; vector canvases export \
                 their VectorFrame directly without rasterisation",
            )
        })?;
        let mut root = Group::default();

        // Background backdrop (drawn first, full-canvas). Solid +
        // Transparent are cheap and could ride the renderer's clear
        // colour, but emitting an explicit node keeps the frame
        // self-describing for the `build_frame` callers above.
        if let Some(node) = background_node(&scene.background, w as f32, h as f32) {
            root.children.push(node);
        }

        // Objects in paint order (z ascending, ties by insertion). We
        // re-borrow each source object by id so we can reach its `kind`
        // payload (the flat `Sample` deliberately omits `kind`).
        //
        // Children listed inside any `ObjectKind::Group` are claimed by
        // their parent group's expansion (see `lower_object`) and
        // skipped at the top level so they don't paint twice.
        let owned: HashSet<ObjectId> = scene
            .objects
            .iter()
            .filter_map(|o| match &o.kind {
                ObjectKind::Group(ids) => Some(ids.iter().copied()),
                _ => None,
            })
            .flatten()
            .collect();

        let ctx = LowerCtx {
            scene,
            t,
            canvas: (w as f32, h as f32),
        };
        for sample in scene.sampled_at(t) {
            if owned.contains(&sample.id) {
                continue;
            }
            let Some(obj) = scene.objects.iter().find(|o| o.id == sample.id) else {
                continue;
            };
            let mut visited = HashSet::new();
            let Some(node) = lower_object(
                &ctx,
                obj,
                SampledState {
                    transform: sample.transform,
                    opacity: sample.opacity,
                    clip: sample.clip,
                },
                &mut visited,
            ) else {
                continue; // unsupported / resource-backed kind — skip.
            };
            root.children.push(node);
        }

        Ok(VectorFrame {
            width: w as f32,
            height: h as f32,
            view_box: None,
            root,
            pts: None,
            time_base: TimeBase::new(1, 1),
        })
    }
}

impl SceneRenderer for RasterRenderer {
    fn prepare(&mut self, scene: &Scene) -> Result<()> {
        // Validate the canvas up front so the first `render_at` failure
        // surfaces at prepare time instead of mid-loop.
        scene.canvas.raster_size().ok_or_else(|| {
            Error::unsupported(
                "oxideav-scene: RasterRenderer needs a Canvas::Raster; vector canvases export \
                 their VectorFrame directly without rasterisation",
            )
        })?;
        Ok(())
    }

    fn render_at(&mut self, scene: &Scene, t: TimeStamp) -> Result<RenderedFrame> {
        let (w, h) = scene.canvas.raster_size().ok_or_else(|| {
            Error::unsupported(
                "oxideav-scene: RasterRenderer needs a Canvas::Raster; vector canvases export \
                 their VectorFrame directly without rasterisation",
            )
        })?;
        let frame = self.build_frame(scene, t)?;
        self.renderer.width = w;
        self.renderer.height = h;
        // Always rasterise onto a transparent clear; the background
        // backdrop node (if any) supplies the solid / gradient fill.
        self.renderer.background = Rgba::new(0, 0, 0, 0);
        let video: VideoFrame = self.renderer.render(&frame);
        Ok(RenderedFrame {
            video: Some(video),
            audio: Vec::new(),
            operations: Vec::new(),
        })
    }

    fn seek(&mut self, _t: TimeStamp) -> Result<()> {
        // RasterRenderer is stateless across timestamps — every
        // `render_at` rebuilds the frame from scratch — so a seek is a
        // no-op. (The bitmap cache stays valid; nothing to invalidate.)
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Background → Node
// ---------------------------------------------------------------------------

/// Build a full-canvas backdrop node for `bg`, or `None` for a
/// transparent background (nothing to paint).
///
/// [`Background::DecodedImage`] lowers symmetrically with the object
/// path (see [`image_node`]): the carried [`VideoFrame`] is wrapped in
/// a [`Node::Image`] whose `bounds` rectangle covers the full canvas,
/// so the downstream rasteriser stretches the source frame across the
/// backdrop. Frames whose first plane doesn't follow the canonical
/// RGBA8-stride convention (`width = stride / 4`,
/// `height = data.len() / stride`) skip silently — the backdrop is
/// then transparent, mirroring [`Background::Transparent`].
fn background_node(bg: &Background, w: f32, h: f32) -> Option<Node> {
    let fill = match bg {
        Background::Transparent => return None,
        Background::Solid(rgba) => CorePaint::Solid(decode_rgba(*rgba)),
        Background::LinearGradient {
            from,
            to,
            angle_deg,
        } => linear_gradient_paint(
            *angle_deg,
            &[Stop::new(0.0, *from), Stop::new(1.0, *to)],
            w,
            h,
        ),
        Background::Gradient(g) => gradient_paint(g, w, h),
        Background::DecodedImage(frame) => {
            // Symmetric with ObjectKind::Image(ImageSource::Decoded):
            // wrap the frame in a Node::Image whose bounds span the
            // full canvas. The downstream raster sampler stretches
            // the source pixels across the backdrop via its
            // configured ImageFilter (bilinear by default), so a
            // non-canvas-sized source frame fills the backdrop with
            // the simplest "stretch to fit" interpretation. Frames
            // that don't follow the RGBA8-stride convention skip
            // silently — the same "drop on degenerate" policy used
            // for object-side images.
            let (fw, fh) = decoded_rgba_size(frame)?;
            if fw == 0 || fh == 0 {
                return None;
            }
            return Some(Node::Image(ImageRef {
                frame: Box::new((**frame).clone()),
                bounds: Rect::new(0.0, 0.0, w, h),
                transform: Transform2D::identity(),
            }));
        }
        // `Background::Image` (filesystem path) needs a decoder the
        // scene crate doesn't bind, and `Background` is
        // `#[non_exhaustive]` so future variants land here too —
        // both are skipped (no backdrop node) until a decoder-aware
        // renderer handles them. Callers with an already-decoded
        // frame in hand should reach for `Background::DecodedImage`
        // above instead.
        _ => return None,
    };
    let path = rect_path(0.0, 0.0, w, h);
    Some(Node::Path(
        PathNode::new(path)
            .with_fill(fill)
            .with_fill_rule(FillRule::NonZero),
    ))
}

// ---------------------------------------------------------------------------
// ObjectKind → Node
// ---------------------------------------------------------------------------

/// Per-frame context carried through the recursive object lowering —
/// the scene + the sample time + the canvas size. Borrowed by every
/// `lower_object` invocation, including the recursive group expansion,
/// so children always agree on the scene under inspection.
struct LowerCtx<'a> {
    scene: &'a Scene,
    t: TimeStamp,
    canvas: (f32, f32),
}

/// Animation-merged per-object state — what the caller's `Sample`
/// carries minus the bits `lower_object` doesn't need. Bundled into a
/// struct so the recursive lowering function stays under
/// `clippy::too_many_arguments`.
struct SampledState {
    transform: Transform,
    opacity: f32,
    clip: Option<ClipRect>,
}

/// Lower a renderable [`SceneObject`] into a vector [`Node`] wrapped in
/// a single [`Group`] carrying the object's animation-merged transform,
/// opacity, and clip. Returns `None` for resource-backed / fully-
/// unparseable kinds — see the module-level skip list.
///
/// `state` is typically passed straight from the caller's `Sample`, but
/// the recursive Group expansion (below) substitutes per-child sampled
/// values so each child's own animations are honoured.
///
/// `visited` tracks object ids already expanded inside this top-level
/// object's subtree so a `Group(vec![id])` that references itself or
/// forms a cycle terminates after the first visit instead of recursing
/// forever.
fn lower_object(
    ctx: &LowerCtx<'_>,
    obj: &SceneObject,
    state: SampledState,
    visited: &mut HashSet<ObjectId>,
) -> Option<Node> {
    if !visited.insert(obj.id) {
        return None; // cycle / repeated visit — drop.
    }
    // Per-object content box: prefer the kind's intrinsic size, fall
    // back to the canvas size (matching the pre-Group code path).
    let (cw, ch) = obj.content_size().unwrap_or(ctx.canvas);
    let matrix = state.transform.to_matrix(cw, ch);
    let clip_path = state.clip.map(|c| rect_path(c.x, c.y, c.width, c.height));

    let children = match &obj.kind {
        ObjectKind::Shape(s) => match shape_node(s) {
            Some(n) => vec![n],
            None => return None,
        },
        ObjectKind::Vector(vf) => vec![Node::Group(vf.root.clone())],
        ObjectKind::Image(src) => match image_node(src) {
            Some(n) => vec![n],
            None => return None,
        },
        ObjectKind::Video(src) => match video_node(src, ctx.t, obj.lifetime.start) {
            Some(n) => vec![n],
            None => return None,
        },
        ObjectKind::Group(ids) => {
            let mut nodes = Vec::new();
            for child_id in ids {
                let Some(child) = ctx.scene.objects.iter().find(|o| &o.id == child_id) else {
                    continue;
                };
                // Honour the child's own lifetime so a group does not
                // promote dead children back into the frame.
                if !child.lifetime.is_live_at(ctx.t) {
                    continue;
                }
                let csample = child.sample_at(ctx.t);
                let mut sub_visited = visited.clone();
                if let Some(node) = lower_object(
                    ctx,
                    child,
                    SampledState {
                        transform: csample.transform,
                        opacity: csample.opacity,
                        clip: csample.clip,
                    },
                    &mut sub_visited,
                ) {
                    nodes.push(node);
                }
            }
            if nodes.is_empty() {
                return None;
            }
            nodes
        }
        // Resource-backed kinds still needing a font face / live
        // source binding the scene crate doesn't carry:
        //
        // * `Live` — pluggable live-source registry.
        // * `Text` — `oxideav_scribe::Face` registry.
        //
        // Pre-decoded `Image(ImageSource::Decoded(_))` is handled
        // above; encoded `Image`/`Video` variants flow through their
        // `image_node` / `video_node` lowerings and fall through to
        // `None` for the decoder-bound arms so they continue to skip
        // cleanly.
        ObjectKind::Text(_) | ObjectKind::Live(_) => return None,
    };

    Some(Node::Group(Group {
        transform: matrix,
        opacity: state.opacity.clamp(0.0, 1.0),
        clip: clip_path,
        children,
        cache_key: None,
    }))
}

/// Lower a [`Shape`] into a filled (+ optionally stroked) [`Node`] in
/// object-local space. [`Shape::Path`] is skipped (opaque SVG data).
fn shape_node(shape: &Shape) -> Option<Node> {
    match shape {
        Shape::Rect {
            width,
            height,
            fill,
            stroke,
            corner_radius,
        } => {
            let path = if *corner_radius > 0.0 {
                rounded_rect_path(*width, *height, *corner_radius)
            } else {
                rect_path(0.0, 0.0, *width, *height)
            };
            Some(fill_stroke_node(path, *fill, stroke.as_ref()))
        }
        Shape::Polygon {
            points,
            fill,
            stroke,
        } => {
            if points.len() < 2 {
                return None;
            }
            let mut path = Path::new();
            path.move_to(Point::new(points[0].0, points[0].1));
            for &(x, y) in &points[1..] {
                path.line_to(Point::new(x, y));
            }
            path.close();
            Some(fill_stroke_node(path, *fill, stroke.as_ref()))
        }
        Shape::Path { data, fill, stroke } => {
            // Parse the SVG path-data string into the core `Path` IR
            // (every SVG 1.1 path command including elliptical arcs
            // `A` / `a`, which lower to `PathCommand::ArcTo` and are
            // flattened to cubics by `oxideav-raster` downstream).
            // Unparseable input is dropped — the renderer never errors
            // a frame on a bad shape; the caller can validate with
            // `crate::svg_path::parse_path` ahead of time if a hard
            // failure is wanted.
            let path = svg_path::parse_path(data).ok()?;
            if path.commands.is_empty() {
                return None;
            }
            Some(fill_stroke_node(path, *fill, stroke.as_ref()))
        }
    }
}

/// Lower an [`ImageSource`] into a [`Node::Image`] sitting in the
/// object's local content box.
///
/// Only [`ImageSource::Decoded`] produces a node — the carried
/// [`oxideav_core::VideoFrame`] is wrapped in an [`ImageRef`] whose
/// `bounds` rectangle spans `(0, 0)..(width, height)` for the frame's
/// natural pixel dimensions (decoded via
/// [`ImageSource::natural_size`]). The downstream
/// [`oxideav_raster::Renderer`] sampler reads pixels through its
/// configured [`oxideav_raster::ImageFilter`].
///
/// [`ImageSource::Path`] and [`ImageSource::EncodedBytes`] return
/// `None` — both still need a decoder binding the scene crate doesn't
/// carry. Callers that already have the decoded pixels in hand should
/// build an `ImageSource::Decoded` instead; the renderer composites
/// them in the same pass as every other supported object kind.
fn image_node(src: &ImageSource) -> Option<Node> {
    let frame = match src {
        ImageSource::Decoded(f) => f,
        // Encoded variants stay decoder-bound; skip silently for now
        // and let a future renderer (with a decoder registry) handle
        // them. `#[non_exhaustive]` makes the wildcard mandatory.
        _ => return None,
    };
    let (w, h) = src.natural_size()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some(Node::Image(ImageRef {
        frame: Box::new((**frame).clone()),
        bounds: Rect::new(0.0, 0.0, w as f32, h as f32),
        transform: Transform2D::identity(),
    }))
}

/// Lower a [`VideoSource`] into a [`Node::Image`] carrying the frame
/// visible at scene time `t` relative to `lifetime_start`.
///
/// Only [`VideoSource::DecodedFrames`] produces a node — the frame is
/// chosen by [`VideoSource::frame_at`], which steps through the
/// sequence at the carried `frame_duration` cadence and clamps to the
/// final frame past the end. The `ImageRef::bounds` rectangle spans
/// `(0, 0)..(width, height)` of the first frame's natural pixel
/// dimensions, matching the `Image` arm so a fixed-resolution
/// sequence composites identically under each object's
/// animation-merged transform / opacity / clip wrap.
///
/// [`VideoSource::Path`] and [`VideoSource::EncodedBytes`] return
/// `None` — both still need a decoder binding the scene crate doesn't
/// carry. Callers that already have the decoded frames in hand should
/// build a `DecodedFrames` instead; the renderer composites them in
/// the same pass as every other supported object kind.
fn video_node(src: &VideoSource, t: TimeStamp, lifetime_start: TimeStamp) -> Option<Node> {
    let frame = src.frame_at(t, lifetime_start)?;
    let (w, h) = src.natural_size()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some(Node::Image(ImageRef {
        frame: Box::new((**frame).clone()),
        bounds: Rect::new(0.0, 0.0, w as f32, h as f32),
        transform: Transform2D::identity(),
    }))
}

/// Assemble a [`PathNode`] with a solid fill and an optional solid
/// stroke. A zero-alpha fill (`fill & 0xff == 0`) yields a fill-less
/// node so a stroke-only shape renders correctly.
fn fill_stroke_node(path: Path, fill: u32, stroke: Option<&Stroke>) -> Node {
    let mut node = PathNode::new(path).with_fill_rule(FillRule::NonZero);
    if (fill & 0xff) != 0 {
        node = node.with_fill(CorePaint::Solid(decode_rgba(fill)));
    }
    if let Some(s) = stroke {
        node = node.with_stroke(CoreStroke::solid(s.width, decode_rgba(s.color)));
    }
    Node::Path(node)
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Axis-aligned rectangle path at `(x, y)` of size `(w, h)`, closed.
fn rect_path(x: f32, y: f32, w: f32, h: f32) -> Path {
    let mut p = Path::new();
    p.move_to(Point::new(x, y));
    p.line_to(Point::new(x + w, y));
    p.line_to(Point::new(x + w, y + h));
    p.line_to(Point::new(x, y + h));
    p.close();
    p
}

/// Rounded-rect path with corner radius `r` (clamped to half the
/// shorter side). Corners are quarter-circle quadratic approximations —
/// good enough for UI panels / lower-thirds; a future round can swap to
/// the cubic kappa approximation if rounder corners are needed.
fn rounded_rect_path(w: f32, h: f32, r: f32) -> Path {
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    if r <= 0.0 {
        return rect_path(0.0, 0.0, w, h);
    }
    let mut p = Path::new();
    // Start after the top-left corner, walk clockwise.
    p.move_to(Point::new(r, 0.0));
    p.line_to(Point::new(w - r, 0.0));
    p.quad_to(Point::new(w, 0.0), Point::new(w, r));
    p.line_to(Point::new(w, h - r));
    p.quad_to(Point::new(w, h), Point::new(w - r, h));
    p.line_to(Point::new(r, h));
    p.quad_to(Point::new(0.0, h), Point::new(0.0, h - r));
    p.line_to(Point::new(0.0, r));
    p.quad_to(Point::new(0.0, 0.0), Point::new(r, 0.0));
    p.close();
    p
}

// ---------------------------------------------------------------------------
// Gradient + colour helpers
// ---------------------------------------------------------------------------

/// Decode a `0xRRGGBBAA` scene colour into [`oxideav_core::Rgba`].
fn decode_rgba(packed: u32) -> Rgba {
    Rgba::new(
        ((packed >> 24) & 0xff) as u8,
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        (packed & 0xff) as u8,
    )
}

/// Build a core [`CorePaint`] for a scene [`Gradient`] spanning a
/// `(w, h)` box (used for the full-canvas backdrop).
fn gradient_paint(g: &Gradient, w: f32, h: f32) -> CorePaint {
    match g {
        Gradient::Linear { angle_deg, stops } => linear_gradient_paint(*angle_deg, stops, w, h),
        Gradient::Radial {
            cx,
            cy,
            radius,
            stops,
        } => {
            // Normalised centre → pixel centre; radius is in units of the
            // smaller axis (per the paint module's convention).
            let center = Point::new(cx * w, cy * h);
            let r = radius * w.min(h);
            let grad = RadialGradient::new(center, r.max(1e-3)).with_stops(core_stops(stops));
            CorePaint::RadialGradient(grad)
        }
    }
}

/// Build a core linear-gradient [`CorePaint`]. `angle_deg` follows the
/// CSS convention used across the paint module: `0°` paints
/// bottom-to-top, `90°` left-to-right, clockwise. The gradient line is
/// the diameter of the `(w, h)` box through its centre at that angle.
fn linear_gradient_paint(angle_deg: f32, stops: &[Stop], w: f32, h: f32) -> CorePaint {
    let theta = angle_deg.to_radians();
    // CSS 0° = upward (toward -y in screen space), increasing clockwise.
    // Direction unit vector of the gradient line:
    let dx = theta.sin();
    let dy = -theta.cos();
    let (cx, cy) = (w * 0.5, h * 0.5);
    // Half-length: project the box half-extent onto the gradient axis so
    // the start/end stops land at the box edge along that direction.
    let half = (dx.abs() * w * 0.5) + (dy.abs() * h * 0.5);
    let start = Point::new(cx - dx * half, cy - dy * half);
    let end = Point::new(cx + dx * half, cy + dy * half);
    let grad = LinearGradient::new(start, end).with_stops(core_stops(stops));
    CorePaint::LinearGradient(grad)
}

/// Convert scene [`Stop`]s into core [`oxideav_core::GradientStop`]s.
fn core_stops(stops: &[Stop]) -> Vec<oxideav_core::GradientStop> {
    stops
        .iter()
        .map(|s| oxideav_core::GradientStop::new(s.offset, decode_rgba(s.color)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ObjectId;
    use crate::object::{Canvas, ClipRect, SceneObject, Transform};
    use crate::scene::Scene;

    fn rect_obj(id: u64, x: f32, y: f32, w: f32, h: f32, fill: u32, z: i32) -> SceneObject {
        SceneObject {
            id: ObjectId::new(id),
            kind: ObjectKind::Shape(Shape::Rect {
                width: w,
                height: h,
                fill,
                stroke: None,
                corner_radius: 0.0,
            }),
            transform: Transform {
                position: (x, y),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            z_order: z,
            ..SceneObject::default()
        }
    }

    fn pixel(frame: &VideoFrame, w: u32, x: u32, y: u32) -> [u8; 4] {
        let off = (y as usize * w as usize + x as usize) * 4;
        let d = &frame.planes[0].data;
        [d[off], d[off + 1], d[off + 2], d[off + 3]]
    }

    #[test]
    fn prepare_rejects_vector_canvas() {
        let scene = Scene {
            canvas: Canvas::Vector {
                width: 100.0,
                height: 100.0,
                unit: crate::object::LengthUnit::Point,
            },
            ..Scene::default()
        };
        let mut r = RasterRenderer::new();
        assert!(matches!(r.prepare(&scene), Err(Error::Unsupported(_))));
        assert!(matches!(r.render_at(&scene, 0), Err(Error::Unsupported(_))));
    }

    #[test]
    fn prepare_accepts_raster_canvas() {
        let scene = Scene {
            canvas: Canvas::raster(16, 16),
            ..Scene::default()
        };
        let mut r = RasterRenderer::new();
        assert!(r.prepare(&scene).is_ok());
    }

    #[test]
    fn renders_solid_background() {
        let scene = Scene {
            canvas: Canvas::raster(8, 8),
            background: Background::Solid(0x112233FF),
            ..Scene::default()
        };
        let mut r = RasterRenderer::new();
        let out = r.render_at(&scene, 0).unwrap();
        let frame = out.video.unwrap();
        assert_eq!(frame.planes[0].data.len(), 8 * 8 * 4);
        // Every pixel should be the background colour.
        let px = pixel(&frame, 8, 4, 4);
        assert_eq!(px, [0x11, 0x22, 0x33, 0xFF]);
    }

    #[test]
    fn transparent_background_leaves_alpha_zero() {
        let scene = Scene {
            canvas: Canvas::raster(8, 8),
            background: Background::Transparent,
            ..Scene::default()
        };
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        // No objects, transparent bg → fully clear.
        let lit = frame.planes[0]
            .data
            .chunks_exact(4)
            .filter(|p| p[3] != 0)
            .count();
        assert_eq!(lit, 0);
    }

    #[test]
    fn renders_filled_rect_at_position() {
        let mut scene = Scene {
            canvas: Canvas::raster(32, 32),
            background: Background::Transparent,
            ..Scene::default()
        };
        // Opaque red 10x10 rect at (5,5), top-left anchor.
        scene
            .objects
            .push(rect_obj(1, 5.0, 5.0, 10.0, 10.0, 0xFF0000FF, 0));
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        // Centre of the rect (10,10) is red+opaque.
        let px = pixel(&frame, 32, 10, 10);
        assert_eq!(px[3], 0xFF, "rect interior should be opaque");
        assert!(
            px[0] > 200 && px[1] < 60 && px[2] < 60,
            "rect should be red: {px:?}"
        );
        // Outside the rect (25,25) is clear.
        assert_eq!(
            pixel(&frame, 32, 25, 25)[3],
            0,
            "outside rect should be clear"
        );
    }

    #[test]
    fn paint_order_later_z_on_top() {
        let mut scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::Transparent,
            ..Scene::default()
        };
        // Red below (z=0), green on top (z=5), overlapping the same cell.
        scene
            .objects
            .push(rect_obj(1, 0.0, 0.0, 16.0, 16.0, 0xFF0000FF, 0));
        scene
            .objects
            .push(rect_obj(2, 0.0, 0.0, 16.0, 16.0, 0x00FF00FF, 5));
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        let px = pixel(&frame, 16, 8, 8);
        assert!(
            px[1] > 200 && px[0] < 60,
            "top (green) object should win: {px:?}"
        );
    }

    #[test]
    fn object_opacity_attenuates_coverage() {
        let mut scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::Solid(0x000000FF),
            ..Scene::default()
        };
        let mut obj = rect_obj(1, 0.0, 0.0, 16.0, 16.0, 0xFFFFFFFF, 0);
        obj.opacity = 0.5;
        scene.objects.push(obj);
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        // White at 0.5 over black → roughly mid-grey.
        let px = pixel(&frame, 16, 8, 8);
        assert!((90..=165).contains(&px[0]), "expected ~grey, got {px:?}");
    }

    #[test]
    fn clip_rect_culls_outside_region() {
        let mut scene = Scene {
            canvas: Canvas::raster(32, 32),
            background: Background::Transparent,
            ..Scene::default()
        };
        let mut obj = rect_obj(1, 0.0, 0.0, 32.0, 32.0, 0xFFFFFFFF, 0);
        // Clip to the top-left 8x8 corner.
        obj.clip = Some(ClipRect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        });
        scene.objects.push(obj);
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        // Inside the clip: lit. Outside: clear.
        assert_eq!(
            pixel(&frame, 32, 2, 2)[3],
            0xFF,
            "inside clip should be lit"
        );
        assert_eq!(
            pixel(&frame, 32, 20, 20)[3],
            0,
            "outside clip should be clear"
        );
    }

    #[test]
    fn linear_gradient_background_varies_across_axis() {
        let scene = Scene {
            canvas: Canvas::raster(32, 4),
            // 90° = left-to-right: black on the left, white on the right.
            background: Background::Gradient(Gradient::linear(
                90.0,
                vec![Stop::new(0.0, 0x000000FF), Stop::new(1.0, 0xFFFFFFFF)],
            )),
            ..Scene::default()
        };
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        let left = pixel(&frame, 32, 1, 2)[0] as i32;
        let right = pixel(&frame, 32, 30, 2)[0] as i32;
        assert!(
            right > left + 80,
            "gradient should brighten L→R: left={left} right={right}"
        );
    }

    #[test]
    fn skips_text_and_encoded_image_kinds_without_error() {
        use crate::object::{ImageSource, TextRun};
        let mut scene = Scene {
            canvas: Canvas::raster(8, 8),
            background: Background::Transparent,
            ..Scene::default()
        };
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Text(TextRun::default()),
            ..SceneObject::default()
        });
        // Encoded variants stay decoder-bound and still skip cleanly —
        // `Decoded` is exercised in `image_object_lights_target_pixels`
        // below.
        scene.objects.push(SceneObject {
            id: ObjectId::new(2),
            kind: ObjectKind::Image(ImageSource::Path("x.png".into())),
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(3),
            kind: ObjectKind::Image(ImageSource::EncodedBytes(
                vec![0x89, 0x50, 0x4e, 0x47].into(),
            )),
            ..SceneObject::default()
        });
        // Encoded Video variants stay decoder-bound too; covered here
        // alongside the encoded Image arms so the "skip without error"
        // contract holds for both sides.
        scene.objects.push(SceneObject {
            id: ObjectId::new(4),
            kind: ObjectKind::Video(VideoSource::Path("x.mp4".into())),
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(5),
            kind: ObjectKind::Video(VideoSource::EncodedBytes(vec![0u8; 8].into())),
            ..SceneObject::default()
        });
        let mut r = RasterRenderer::new();
        // Renders cleanly (skipped kinds contribute nothing).
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        let lit = frame.planes[0]
            .data
            .chunks_exact(4)
            .filter(|p| p[3] != 0)
            .count();
        assert_eq!(lit, 0);
    }

    /// Helper: build a solid-colour RGBA8 [`VideoFrame`] of the given
    /// pixel dimensions, packed under the canonical
    /// `stride = width * 4` convention that the rest of the pipeline
    /// (raster sampler, scene `Decoded` natural-size decoding) reads.
    fn solid_rgba_frame(width: u32, height: u32, rgba: [u8; 4]) -> VideoFrame {
        use oxideav_core::VideoPlane;
        let stride = (width as usize) * 4;
        let mut data = Vec::with_capacity(stride * height as usize);
        for _ in 0..(width as usize * height as usize) {
            data.extend_from_slice(&rgba);
        }
        VideoFrame {
            pts: None,
            planes: vec![VideoPlane { stride, data }],
        }
    }

    #[test]
    fn decoded_image_source_reports_natural_size() {
        use std::sync::Arc;
        let frame = solid_rgba_frame(7, 5, [0xFF, 0x00, 0x00, 0xFF]);
        let src = ImageSource::Decoded(Arc::new(frame));
        assert_eq!(src.natural_size(), Some((7, 5)));
        assert_eq!(
            ImageSource::Path("x.png".into()).natural_size(),
            None,
            "encoded path needs a decoder"
        );
        assert_eq!(
            ImageSource::EncodedBytes(vec![0u8; 4].into()).natural_size(),
            None,
            "encoded bytes need a decoder"
        );
    }

    #[test]
    fn image_object_emits_image_node_under_object_transform() {
        use std::sync::Arc;
        let mut scene = Scene {
            canvas: Canvas::raster(32, 32),
            background: Background::Transparent,
            ..Scene::default()
        };
        let frame = solid_rgba_frame(4, 4, [0x12, 0x34, 0x56, 0xFF]);
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Image(ImageSource::Decoded(Arc::new(frame))),
            transform: Transform {
                position: (10.0, 10.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        });
        let r = RasterRenderer::new();
        let vf = r.build_frame(&scene, 0).unwrap();
        // No background → only the object group on root.
        assert_eq!(vf.root.children.len(), 1, "expected only the image group");
        let Node::Group(g) = &vf.root.children[0] else {
            panic!("expected a Group wrapper around the image");
        };
        assert_eq!(g.children.len(), 1, "expected a single Node::Image child");
        match &g.children[0] {
            Node::Image(img) => {
                assert_eq!(
                    (img.bounds.width as u32, img.bounds.height as u32),
                    (4, 4),
                    "bounds should match the source frame's natural size"
                );
                assert_eq!(img.bounds.x, 0.0);
                assert_eq!(img.bounds.y, 0.0);
            }
            other => panic!("expected Node::Image, got {other:?}"),
        }
    }

    #[test]
    fn image_object_lights_target_pixels() {
        use std::sync::Arc;
        let mut scene = Scene {
            canvas: Canvas::raster(32, 32),
            background: Background::Transparent,
            ..Scene::default()
        };
        // 8×8 opaque blue source frame, placed at (10,10) top-left
        // anchor — so canvas pixels [10..18, 10..18) should pick up
        // the blue. Pixels outside stay clear.
        let frame = solid_rgba_frame(8, 8, [0x00, 0x00, 0xFF, 0xFF]);
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Image(ImageSource::Decoded(Arc::new(frame))),
            transform: Transform {
                position: (10.0, 10.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        });
        let mut r = RasterRenderer::new();
        let out = r.render_at(&scene, 0).unwrap().video.unwrap();
        // Interior pixel — should be opaque blue. The bilinear sampler
        // matches the texel exactly at integer-centred lookups.
        let inside = pixel(&out, 32, 13, 13);
        assert_eq!(
            inside[3], 0xFF,
            "image interior should be opaque, got {inside:?}"
        );
        assert!(
            inside[2] > 200 && inside[0] < 40 && inside[1] < 40,
            "image interior should be blue, got {inside:?}"
        );
        // Pixel outside the placed image stays transparent.
        let outside = pixel(&out, 32, 25, 25);
        assert_eq!(
            outside[3], 0,
            "outside image bounds should stay clear, got {outside:?}"
        );
        // Pixel before the image starts on the X axis stays clear too.
        let before = pixel(&out, 32, 2, 13);
        assert_eq!(
            before[3], 0,
            "outside image bounds should stay clear, got {before:?}"
        );
    }

    #[test]
    fn image_object_honours_opacity() {
        use std::sync::Arc;
        let mut scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::Solid(0x000000FF),
            ..Scene::default()
        };
        let frame = solid_rgba_frame(16, 16, [0xFF, 0xFF, 0xFF, 0xFF]);
        let mut obj = SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Image(ImageSource::Decoded(Arc::new(frame))),
            transform: Transform {
                position: (0.0, 0.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        };
        obj.opacity = 0.5;
        scene.objects.push(obj);
        let mut r = RasterRenderer::new();
        let out = r.render_at(&scene, 0).unwrap().video.unwrap();
        // White at 0.5 over black → roughly mid-grey, mirroring the
        // existing `object_opacity_attenuates_coverage` shape test.
        let px = pixel(&out, 16, 8, 8);
        assert!(
            (90..=165).contains(&px[0]),
            "expected ~grey from a half-opaque image, got {px:?}"
        );
    }

    #[test]
    fn image_object_honours_clip_rect() {
        use std::sync::Arc;
        let mut scene = Scene {
            canvas: Canvas::raster(32, 32),
            background: Background::Transparent,
            ..Scene::default()
        };
        let frame = solid_rgba_frame(32, 32, [0xFF, 0xFF, 0xFF, 0xFF]);
        let mut obj = SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Image(ImageSource::Decoded(Arc::new(frame))),
            transform: Transform {
                position: (0.0, 0.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        };
        obj.clip = Some(ClipRect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        });
        scene.objects.push(obj);
        let mut r = RasterRenderer::new();
        let out = r.render_at(&scene, 0).unwrap().video.unwrap();
        // Inside the clip: lit. Outside: clear — same shape as the
        // `clip_rect_culls_outside_region` test on rects.
        assert_eq!(pixel(&out, 32, 2, 2)[3], 0xFF, "inside clip should be lit");
        assert_eq!(
            pixel(&out, 32, 20, 20)[3],
            0,
            "outside clip should be clear"
        );
    }

    #[test]
    fn image_inside_group_renders_under_parent_transform() {
        use std::sync::Arc;
        let mut scene = Scene {
            canvas: Canvas::raster(32, 32),
            background: Background::Transparent,
            ..Scene::default()
        };
        let frame = solid_rgba_frame(4, 4, [0x00, 0xFF, 0x00, 0xFF]);
        // Child image at (0,0) — would normally land in the top-left
        // corner. Group offsets it by (12, 12).
        scene.objects.push(SceneObject {
            id: ObjectId::new(10),
            kind: ObjectKind::Image(ImageSource::Decoded(Arc::new(frame))),
            transform: Transform {
                position: (0.0, 0.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(20),
            kind: ObjectKind::Group(vec![ObjectId::new(10)]),
            transform: Transform {
                position: (12.0, 12.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        });
        let mut r = RasterRenderer::new();
        let out = r.render_at(&scene, 0).unwrap().video.unwrap();
        // Inside the parent-shifted image (12..16, 12..16) — green.
        let inside = pixel(&out, 32, 13, 13);
        assert_eq!(inside[3], 0xFF, "expected lit pixel, got {inside:?}");
        assert!(
            inside[1] > 200 && inside[0] < 40 && inside[2] < 40,
            "expected green pixel, got {inside:?}"
        );
        // (1, 1) — where the un-grouped child would have painted — is
        // clear, confirming the group transform reached the image.
        assert_eq!(pixel(&out, 32, 1, 1)[3], 0);
    }

    #[test]
    fn build_frame_emits_node_per_supported_object() {
        let mut scene = Scene {
            canvas: Canvas::raster(64, 64),
            background: Background::Solid(0x000000FF),
            ..Scene::default()
        };
        scene
            .objects
            .push(rect_obj(1, 0.0, 0.0, 8.0, 8.0, 0xFFFFFFFF, 0));
        scene
            .objects
            .push(rect_obj(2, 10.0, 10.0, 8.0, 8.0, 0xFFFFFFFF, 1));
        let r = RasterRenderer::new();
        let frame = r.build_frame(&scene, 0).unwrap();
        // 1 background backdrop + 2 object groups.
        assert_eq!(frame.root.children.len(), 3);
    }

    #[test]
    fn decoded_image_background_emits_full_canvas_image_node() {
        use std::sync::Arc;
        let bg_frame = solid_rgba_frame(4, 4, [0x10, 0x20, 0x30, 0xFF]);
        let scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::DecodedImage(Arc::new(bg_frame)),
            ..Scene::default()
        };
        let r = RasterRenderer::new();
        let vf = r.build_frame(&scene, 0).unwrap();
        // Only the backdrop node, no objects.
        assert_eq!(vf.root.children.len(), 1);
        match &vf.root.children[0] {
            Node::Image(img) => {
                // Backdrop bounds span the full canvas.
                assert_eq!(img.bounds.x, 0.0);
                assert_eq!(img.bounds.y, 0.0);
                assert_eq!(img.bounds.width as u32, 16);
                assert_eq!(img.bounds.height as u32, 16);
            }
            other => panic!("expected Node::Image backdrop, got {other:?}"),
        }
    }

    #[test]
    fn decoded_image_background_lights_target_pixels() {
        use std::sync::Arc;
        // 8×8 opaque red source — backdrop should stretch across the
        // 16×16 canvas, so every canvas pixel reports red+opaque.
        let bg_frame = solid_rgba_frame(8, 8, [0xFF, 0x00, 0x00, 0xFF]);
        let scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::DecodedImage(Arc::new(bg_frame)),
            ..Scene::default()
        };
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        // Mid-canvas pixel — should be opaque red after the bilinear
        // stretch of the constant-colour source.
        let centre = pixel(&frame, 16, 8, 8);
        assert_eq!(centre[3], 0xFF, "backdrop centre should be opaque");
        assert!(
            centre[0] > 200 && centre[1] < 40 && centre[2] < 40,
            "backdrop centre should be red, got {centre:?}"
        );
        // Corner pixel — should also be red (a constant-colour source
        // stretches to a constant-colour backdrop edge to edge).
        let corner = pixel(&frame, 16, 0, 0);
        assert_eq!(corner[3], 0xFF, "backdrop corner should be opaque");
        assert!(
            corner[0] > 200 && corner[1] < 40 && corner[2] < 40,
            "backdrop corner should be red, got {corner:?}"
        );
    }

    #[test]
    fn decoded_image_background_composites_under_objects() {
        use std::sync::Arc;
        // Red 16×16 backdrop; green 8×8 object at (4,4). The object
        // wins inside its rectangle (paint order: backdrop first).
        let bg_frame = solid_rgba_frame(16, 16, [0xFF, 0x00, 0x00, 0xFF]);
        let mut scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::DecodedImage(Arc::new(bg_frame)),
            ..Scene::default()
        };
        scene
            .objects
            .push(rect_obj(1, 4.0, 4.0, 8.0, 8.0, 0x00FF00FF, 0));
        let mut r = RasterRenderer::new();
        let out = r.render_at(&scene, 0).unwrap().video.unwrap();
        // Inside the rect — green wins.
        let inside = pixel(&out, 16, 8, 8);
        assert!(
            inside[1] > 200 && inside[0] < 40,
            "object should win over backdrop, got {inside:?}"
        );
        // Outside the rect — backdrop red shows through.
        let outside = pixel(&out, 16, 1, 1);
        assert!(
            outside[0] > 200 && outside[1] < 40 && outside[2] < 40,
            "backdrop should show outside object bounds, got {outside:?}"
        );
    }

    #[test]
    fn degenerate_decoded_image_background_skips_silently() {
        use oxideav_core::VideoPlane;
        use std::sync::Arc;
        // A "frame" whose first plane carries a stride that doesn't
        // divide cleanly into the data length — fails the
        // RGBA8-stride round-trip and should drop the backdrop without
        // erroring the render (no node emitted; canvas stays clear).
        let bad = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 16,
                data: vec![0u8; 17], // 17 % 16 != 0
            }],
        };
        let scene = Scene {
            canvas: Canvas::raster(8, 8),
            background: Background::DecodedImage(Arc::new(bad)),
            ..Scene::default()
        };
        let r = RasterRenderer::new();
        let vf = r.build_frame(&scene, 0).unwrap();
        assert_eq!(
            vf.root.children.len(),
            0,
            "degenerate backdrop should drop without erroring"
        );
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        let lit = frame.planes[0]
            .data
            .chunks_exact(4)
            .filter(|p| p[3] != 0)
            .count();
        assert_eq!(lit, 0, "no backdrop → canvas stays fully clear");
    }

    #[test]
    fn path_background_image_still_skips_silently() {
        // The path-based variant continues to need a decoder binding
        // the scene crate doesn't carry; the backdrop drops cleanly.
        let scene = Scene {
            canvas: Canvas::raster(8, 8),
            background: Background::Image("nonexistent.png".into()),
            ..Scene::default()
        };
        let r = RasterRenderer::new();
        let vf = r.build_frame(&scene, 0).unwrap();
        assert_eq!(vf.root.children.len(), 0);
    }

    #[test]
    fn seek_is_noop_ok() {
        let mut r = RasterRenderer::new();
        assert!(r.seek(123).is_ok());
    }

    // -----------------------------------------------------------------
    // VideoSource::DecodedFrames — pre-decoded sequence composition.
    // -----------------------------------------------------------------

    #[test]
    fn decoded_video_source_reports_first_frame_natural_size() {
        use crate::duration::TimeStamp;
        use std::sync::Arc;
        let f0 = solid_rgba_frame(6, 4, [0x10, 0x20, 0x30, 0xFF]);
        let f1 = solid_rgba_frame(6, 4, [0x40, 0x50, 0x60, 0xFF]);
        let src = VideoSource::DecodedFrames {
            frames: vec![Arc::new(f0), Arc::new(f1)],
            frame_duration: 10 as TimeStamp,
        };
        assert_eq!(src.natural_size(), Some((6, 4)));
        // Path / EncodedBytes still need a decoder.
        assert_eq!(VideoSource::Path("x.mp4".into()).natural_size(), None);
        assert_eq!(
            VideoSource::EncodedBytes(vec![0u8; 4].into()).natural_size(),
            None
        );
    }

    #[test]
    fn empty_decoded_video_source_skips_silently() {
        use crate::duration::TimeStamp;
        let mut scene = Scene {
            canvas: Canvas::raster(8, 8),
            background: Background::Transparent,
            ..Scene::default()
        };
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Video(VideoSource::DecodedFrames {
                frames: Vec::new(),
                frame_duration: 1 as TimeStamp,
            }),
            ..SceneObject::default()
        });
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        let lit = frame.planes[0]
            .data
            .chunks_exact(4)
            .filter(|p| p[3] != 0)
            .count();
        assert_eq!(lit, 0, "empty video sequence emits no pixels");
    }

    #[test]
    fn video_object_emits_image_node_under_object_transform() {
        use crate::duration::TimeStamp;
        use std::sync::Arc;
        let mut scene = Scene {
            canvas: Canvas::raster(32, 32),
            background: Background::Transparent,
            ..Scene::default()
        };
        let f0 = solid_rgba_frame(4, 4, [0x12, 0x34, 0x56, 0xFF]);
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Video(VideoSource::DecodedFrames {
                frames: vec![Arc::new(f0)],
                frame_duration: 10 as TimeStamp,
            }),
            transform: Transform {
                position: (10.0, 10.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        });
        let r = RasterRenderer::new();
        let vf = r.build_frame(&scene, 0).unwrap();
        assert_eq!(vf.root.children.len(), 1, "expected only the video group");
        let Node::Group(g) = &vf.root.children[0] else {
            panic!("expected a Group wrapper around the video frame");
        };
        assert_eq!(g.children.len(), 1, "expected a single Node::Image child");
        match &g.children[0] {
            Node::Image(img) => {
                assert_eq!(
                    (img.bounds.width as u32, img.bounds.height as u32),
                    (4, 4),
                    "bounds should match the first frame's natural size"
                );
                assert_eq!(img.bounds.x, 0.0);
                assert_eq!(img.bounds.y, 0.0);
            }
            other => panic!("expected Node::Image, got {other:?}"),
        }
    }

    #[test]
    fn video_object_advances_through_frames_with_scene_time() {
        use crate::duration::TimeStamp;
        use std::sync::Arc;
        // Two-frame sequence, frame_duration = 10 ticks. At t=5 the
        // first (red) frame is visible; at t=15 the second (green)
        // frame is visible.
        let red = solid_rgba_frame(8, 8, [0xFF, 0x00, 0x00, 0xFF]);
        let green = solid_rgba_frame(8, 8, [0x00, 0xFF, 0x00, 0xFF]);
        let make_scene = || {
            let mut s = Scene {
                canvas: Canvas::raster(16, 16),
                background: Background::Transparent,
                ..Scene::default()
            };
            s.objects.push(SceneObject {
                id: ObjectId::new(1),
                kind: ObjectKind::Video(VideoSource::DecodedFrames {
                    frames: vec![Arc::new(red.clone()), Arc::new(green.clone())],
                    frame_duration: 10 as TimeStamp,
                }),
                transform: Transform {
                    position: (4.0, 4.0),
                    anchor: (0.0, 0.0),
                    ..Transform::identity()
                },
                ..SceneObject::default()
            });
            s
        };
        let scene = make_scene();
        let mut r = RasterRenderer::new();
        let early = r.render_at(&scene, 5).unwrap().video.unwrap();
        let mid = pixel(&early, 16, 7, 7);
        assert_eq!(mid[3], 0xFF, "early-sample interior should be opaque");
        assert!(
            mid[0] > 200 && mid[1] < 40 && mid[2] < 40,
            "early sample should be red (frame 0), got {mid:?}"
        );
        let late = r.render_at(&scene, 15).unwrap().video.unwrap();
        let mid = pixel(&late, 16, 7, 7);
        assert_eq!(mid[3], 0xFF, "late-sample interior should be opaque");
        assert!(
            mid[1] > 200 && mid[0] < 40 && mid[2] < 40,
            "late sample should be green (frame 1), got {mid:?}"
        );
    }

    #[test]
    fn video_object_clamps_past_end_to_final_frame() {
        use crate::duration::TimeStamp;
        use std::sync::Arc;
        // Two-frame sequence; sampling well past the end keeps the
        // final (green) frame visible until the lifetime expires.
        let red = solid_rgba_frame(8, 8, [0xFF, 0x00, 0x00, 0xFF]);
        let green = solid_rgba_frame(8, 8, [0x00, 0xFF, 0x00, 0xFF]);
        let mut scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::Transparent,
            ..Scene::default()
        };
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Video(VideoSource::DecodedFrames {
                frames: vec![Arc::new(red), Arc::new(green)],
                frame_duration: 10 as TimeStamp,
            }),
            transform: Transform {
                position: (4.0, 4.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        });
        let mut r = RasterRenderer::new();
        let way_past = r.render_at(&scene, 10_000).unwrap().video.unwrap();
        let px = pixel(&way_past, 16, 7, 7);
        assert_eq!(px[3], 0xFF, "tail frame should still be opaque");
        assert!(
            px[1] > 200 && px[0] < 40 && px[2] < 40,
            "tail should hold on final green frame, got {px:?}"
        );
    }

    #[test]
    fn video_object_honours_lifetime_start_offset() {
        use crate::duration::Lifetime;
        use crate::duration::TimeStamp;
        use std::sync::Arc;
        // Lifetime starts at t=100; the sequence's own t=0 is mapped
        // there. At scene t=105 we should see frame 0 (red); at t=115
        // frame 1 (green).
        let red = solid_rgba_frame(8, 8, [0xFF, 0x00, 0x00, 0xFF]);
        let green = solid_rgba_frame(8, 8, [0x00, 0xFF, 0x00, 0xFF]);
        let mut scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::Transparent,
            ..Scene::default()
        };
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Video(VideoSource::DecodedFrames {
                frames: vec![Arc::new(red), Arc::new(green)],
                frame_duration: 10 as TimeStamp,
            }),
            transform: Transform {
                position: (4.0, 4.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            lifetime: Lifetime {
                start: 100,
                end: None,
            },
            ..SceneObject::default()
        });
        let mut r = RasterRenderer::new();
        let f0 = r.render_at(&scene, 105).unwrap().video.unwrap();
        let p0 = pixel(&f0, 16, 7, 7);
        assert!(
            p0[0] > 200 && p0[1] < 40,
            "t=105 should show red frame 0, got {p0:?}"
        );
        let f1 = r.render_at(&scene, 115).unwrap().video.unwrap();
        let p1 = pixel(&f1, 16, 7, 7);
        assert!(
            p1[1] > 200 && p1[0] < 40,
            "t=115 should show green frame 1, got {p1:?}"
        );
    }

    #[test]
    fn degenerate_decoded_video_frame_skips_silently() {
        use crate::duration::TimeStamp;
        use oxideav_core::VideoPlane;
        use std::sync::Arc;
        // First frame has a stride that doesn't divide cleanly into
        // its data length — fails the RGBA8-stride round-trip; the
        // renderer should drop the object instead of erroring.
        let bad = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 16,
                data: vec![0u8; 17],
            }],
        };
        let mut scene = Scene {
            canvas: Canvas::raster(8, 8),
            background: Background::Transparent,
            ..Scene::default()
        };
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Video(VideoSource::DecodedFrames {
                frames: vec![Arc::new(bad)],
                frame_duration: 10 as TimeStamp,
            }),
            ..SceneObject::default()
        });
        let r = RasterRenderer::new();
        let vf = r.build_frame(&scene, 0).unwrap();
        assert_eq!(
            vf.root.children.len(),
            0,
            "degenerate first frame should drop without erroring"
        );
        let mut r = RasterRenderer::new();
        let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
        let lit = frame.planes[0]
            .data
            .chunks_exact(4)
            .filter(|p| p[3] != 0)
            .count();
        assert_eq!(lit, 0, "no node → canvas stays fully clear");
    }

    #[test]
    fn video_object_honours_opacity() {
        use crate::duration::TimeStamp;
        use std::sync::Arc;
        let mut scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::Solid(0x000000FF),
            ..Scene::default()
        };
        let frame = solid_rgba_frame(16, 16, [0xFF, 0xFF, 0xFF, 0xFF]);
        let mut obj = SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Video(VideoSource::DecodedFrames {
                frames: vec![Arc::new(frame)],
                frame_duration: 10 as TimeStamp,
            }),
            transform: Transform {
                position: (0.0, 0.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        };
        obj.opacity = 0.5;
        scene.objects.push(obj);
        let mut r = RasterRenderer::new();
        let out = r.render_at(&scene, 0).unwrap().video.unwrap();
        let px = pixel(&out, 16, 8, 8);
        assert!(
            (90..=165).contains(&px[0]),
            "expected ~grey from a half-opaque video frame, got {px:?}"
        );
    }

    #[test]
    fn zero_frame_duration_holds_on_first_frame() {
        use crate::duration::TimeStamp;
        use std::sync::Arc;
        // A degenerate `frame_duration` of 0 should not divide-by-zero;
        // the renderer holds on frame 0 regardless of scene time.
        let red = solid_rgba_frame(8, 8, [0xFF, 0x00, 0x00, 0xFF]);
        let mut scene = Scene {
            canvas: Canvas::raster(16, 16),
            background: Background::Transparent,
            ..Scene::default()
        };
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            kind: ObjectKind::Video(VideoSource::DecodedFrames {
                frames: vec![Arc::new(red)],
                frame_duration: 0 as TimeStamp,
            }),
            transform: Transform {
                position: (4.0, 4.0),
                anchor: (0.0, 0.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        });
        let mut r = RasterRenderer::new();
        let out = r.render_at(&scene, 1_000_000).unwrap().video.unwrap();
        let px = pixel(&out, 16, 7, 7);
        assert!(
            px[0] > 200 && px[1] < 40 && px[2] < 40,
            "frame_duration=0 should clamp to frame 0 (red), got {px:?}"
        );
    }
}
