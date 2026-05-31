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
//!   `Path` (SVG path data parsed via [`crate::svg_path::parse_path`];
//!   unparseable data — including arcs — is skipped without error).
//! * [`ObjectKind::Vector`] — the carried [`oxideav_core::VectorFrame`]'s
//!   root group is inlined under the object's transform.
//! * [`ObjectKind::Group`] — child object ids are resolved against the
//!   scene and inlined under the group's own `Transform` / `opacity` /
//!   `clip` (composed multiplicatively over each child's own
//!   sampled state). Cycles in the child graph are broken at the
//!   second visit (each id rendered at most once per group expansion);
//!   missing ids are silently dropped.
//!
//! The kinds that need a decoder or a font face are **skipped** (they
//! contribute nothing to the frame, but never error):
//!
//! * [`ObjectKind::Image`] / [`ObjectKind::Video`] / [`ObjectKind::Live`]
//!   — need a decoder / live source the scene crate doesn't bind. A
//!   future renderer composites the pulled frame via
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
    Error, FillRule, Group, LinearGradient, Node, Paint as CorePaint, Path, PathNode, Point,
    RadialGradient, Result, Rgba, Stroke as CoreStroke, TimeBase, VectorFrame, VideoFrame,
};
use oxideav_raster::Renderer;

use std::collections::HashSet;

use crate::duration::TimeStamp;
use crate::id::ObjectId;
use crate::object::{ClipRect, ObjectKind, SceneObject, Shape, Stroke, Transform};
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
        // `Background::Image` needs a decoder, and `Background` is
        // `#[non_exhaustive]` so future variants land here too — both
        // are skipped (no backdrop node) until a decoder-aware renderer
        // handles them.
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
        // Resource-backed kinds: skipped (still need a decoder / font
        // face binding).
        ObjectKind::Image(_) | ObjectKind::Video(_) | ObjectKind::Text(_) | ObjectKind::Live(_) => {
            return None
        }
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
            // Parse the SVG path-data string into the core `Path` IR.
            // Unparseable input (including the unsupported arc command)
            // is dropped — the renderer never errors a frame on a bad
            // shape; the caller can validate with
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
    fn skips_text_and_image_kinds_without_error() {
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
        scene.objects.push(SceneObject {
            id: ObjectId::new(2),
            kind: ObjectKind::Image(ImageSource::Path("x.png".into())),
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
    fn seek_is_noop_ok() {
        let mut r = RasterRenderer::new();
        assert!(r.seek(123).is_ok());
    }
}
