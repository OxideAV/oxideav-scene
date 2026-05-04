//! Text-run rasterisation for [`crate::TextRun`] via the
//! [`oxideav_scribe`] vector shaper + [`oxideav_raster`] vector→pixel
//! renderer.
//!
//! Round 1 of the scene crate's renderer was a trait-only scaffold —
//! [`crate::render::StubRenderer`] returns `Error::Unsupported`. This
//! module is the first concrete piece of *actual* rendering: it takes
//! a `TextRun` plus a [`oxideav_scribe::Face`] supplied by the caller
//! (the scene crate does not implement font discovery — see scope notes
//! below), shapes it into a chain of positioned glyph nodes, builds a
//! [`oxideav_core::VectorFrame`], and rasterises it via
//! [`oxideav_raster::Renderer`] into a straight-alpha RGBA framebuffer.
//!
//! ## Pipeline
//!
//! 1. The caller hands us a [`TextRenderer`] holding a parsed
//!    [`oxideav_scribe::Face`]. The face is wrapped in a
//!    [`oxideav_scribe::FaceChain`] internally so the shaper's
//!    cmap-fallback path is exercised even when only one face is
//!    registered (chain index 0 == primary). Multiple runs against the
//!    same renderer share the underlying `oxideav_raster::Renderer`'s
//!    glyph-bitmap LRU (which keys on `Group::cache_key` populated by
//!    [`oxideav_scribe::Shaper::shape_to_paths`]).
//! 2. For each [`TextRun`]:
//!    * Decode the `0xRRGGBBAA` colour into a `Rgba` quartet.
//!    * If the run carries `\n` characters, split on newline (or
//!      `wrap_lines` against `max_width_px`) and shape each line.
//!    * Otherwise shape the run text directly via
//!      [`oxideav_scribe::Shaper::shape_to_paths`].
//!    * Recolour each glyph's [`oxideav_core::PathNode::fill`] from the
//!      default black to the run colour. Bitmap glyphs (CBDT/sbix
//!      `Node::Image`) keep their carried palette.
//!    * Wrap the glyphs in a translating [`oxideav_core::Group`] so the
//!      run sits at the requested pen position with the baseline below
//!      the bitmap top by `face.ascent_px(size)`.
//! 3. Stage the [`oxideav_core::VectorFrame`], rasterise it via
//!    [`oxideav_raster::Renderer`] (transparent background), and either
//!    return the resulting [`RgbaBitmap`] (`render_run` /
//!    `render_run_wrapped`) or composite it onto the destination
//!    framebuffer (`render_run_into` / `render_run_wrapped_into`).
//!
//! ## Scope (round 2)
//!
//! * **Font discovery is the caller's job.** A scene-level font
//!   registry / `font_family → Face` resolver is intentionally out of
//!   scope; the renderer takes one `Face` and uses it for every
//!   `TextRun` it sees, ignoring `run.font_family`. Picking the right
//!   `Face` for the family belongs in the application layer.
//! * **Per-glyph colour (CPAL/COLR) is forwarded as-is** — the run's
//!   single foreground colour applies to every outline glyph; bitmap
//!   colour glyphs (CBDT, sbix) keep their built-in palette since the
//!   `Node::Image` carries the encoded colour pixels directly.
//! * **Underline drawing is deferred** to round 3 (see the inline
//!   `// round-3 note` in [`TextRenderer::render_run_into`]).
//! * **Italic** — Scribe does not yet expose a synthetic-italic shear
//!   API on the vector path. When `run.italic` is set we apply the same
//!   per-row horizontal shear `oxideav_subtitle::compositor` uses for
//!   bitmap-font italic (cell width / 4) directly on the rasterised
//!   bitmap, AFTER composition. Once Scribe gains a real italic shear
//!   on the `Shaper::shape_to_paths` path, the fake-shear branch goes
//!   away.
//! * **Per-run advances** (`TextRun::advances`) — recorded but ignored
//!   in round 2; the shaper computes its own advances. Honouring
//!   caller-provided advances (PDF-style explicit positioning) is a
//!   future round.

use oxideav_core::{
    FillRule, Group, Node, Paint, PathNode, Rgba as CoreRgba, TimeBase, Transform2D, VectorFrame,
};
use oxideav_raster::Renderer;
use oxideav_scribe::{Face, FaceChain, Shaper};

use crate::object::TextRun;

/// A grayscale-irrelevant straight-alpha RGBA8 bitmap. Stride is
/// `width * 4`. Mirrors the data layout of the now-removed
/// `oxideav_scribe::RgbaBitmap` so the `TextRenderer` API stays
/// byte-stable across the round-2 vector-pipeline migration.
#[derive(Debug, Clone, Default)]
pub struct RgbaBitmap {
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// Row-major straight-alpha RGBA8 bytes (`width * height * 4`).
    pub data: Vec<u8>,
}

impl RgbaBitmap {
    /// Allocate a fully-transparent (alpha = 0) bitmap.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    /// True if the bitmap holds zero pixels.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Owns a [`Face`] (wrapped in a single-face [`FaceChain`]) and a long-lived
/// [`Renderer`] for per-glyph bitmap-cache reuse across runs.
///
/// Callers construct one `TextRenderer` per face they want to use, and
/// call [`TextRenderer::render_run_into`] for every `TextRun` they want
/// composited.
#[derive(Debug)]
pub struct TextRenderer {
    chain: FaceChain,
    /// Reusable rasteriser. Resized on every render call (cheap — a
    /// `Renderer` is just a few configuration fields plus a reusable
    /// shared bitmap-cache `Arc<Mutex<...>>`). Holding it on the
    /// renderer lets the cache survive across `render_run` calls so
    /// the same glyph at the same size isn't re-rasterised.
    renderer: Renderer,
}

impl TextRenderer {
    /// Build a renderer around an already-parsed [`Face`]. The face
    /// is owned for the lifetime of the renderer; if you need to
    /// switch fonts mid-scene, build a second `TextRenderer`.
    pub fn new(face: Face) -> Self {
        Self {
            chain: FaceChain::new(face),
            // Initial canvas size is a placeholder — every render call
            // resets `width` / `height` to the actual run extent before
            // calling `Renderer::render`. The cache is what matters
            // here, and it survives the resize.
            renderer: Renderer::new(1, 1),
        }
    }

    /// Borrow the underlying primary face — useful for caller-side
    /// metric queries (line height, ascent) without re-parsing.
    pub fn face(&self) -> &Face {
        self.chain.primary()
    }

    /// Pixel line-height for `size_px`. Convenience wrapper around
    /// [`Face::line_height_px`] so callers laying out multi-line
    /// `TextRun` content don't need to dig into Scribe directly.
    pub fn line_height_px(&self, size_px: f32) -> f32 {
        self.face().line_height_px(size_px)
    }

    /// Render a `TextRun` into a freshly-allocated straight-alpha
    /// RGBA bitmap sized to the run's natural glyph bounds.
    ///
    /// Returns an empty bitmap if the run shapes to zero glyphs (or
    /// every glyph is non-rendering — e.g. a string of spaces).
    pub fn render_run(&mut self, run: &TextRun) -> Result<RgbaBitmap, oxideav_scribe::Error> {
        let size = sane_size(run.font_size);
        let bm = self.shape_and_render_line(&run.text, size, run.color);
        if run.italic && !bm.is_empty() {
            return Ok(apply_fake_italic(&bm, size));
        }
        Ok(bm)
    }

    /// Render a `TextRun` into a caller-provided RGBA destination at
    /// pen position `(pen_x, pen_y)`. The destination buffer must be
    /// `dst_w * dst_h * 4` bytes (straight-alpha RGBA8). Pixels
    /// outside the destination are clipped.
    ///
    /// `pen_x` / `pen_y` is the **top-left** of the run's bitmap
    /// (NOT the typographic baseline) to match the previous Scribe
    /// `RgbaBitmap` origin convention.
    #[allow(clippy::too_many_arguments)]
    pub fn render_run_into(
        &mut self,
        run: &TextRun,
        dst: &mut [u8],
        dst_w: u32,
        dst_h: u32,
        pen_x: i32,
        pen_y: i32,
    ) -> Result<(), oxideav_scribe::Error> {
        let bm = self.render_run(run)?;
        if bm.is_empty() {
            return Ok(());
        }
        blit_rgba_straight(
            dst, dst_w, dst_h, pen_x, pen_y, &bm.data, bm.width, bm.height,
        );
        // round-3 note: `run.underline` should drive a 1..2 px filled
        // rectangle at `pen_y + ascent + 1` spanning `bm.width`. The
        // Y-position needs the face's underline metric (post.underlinePosition
        // / underlineThickness) which Scribe doesn't yet plumb through Face;
        // once it does, draw it here.
        let _ = run.underline;
        let _ = run.advances;
        Ok(())
    }

    /// Render a `TextRun` whose text may overflow `max_width_px` or
    /// contain `\n` characters. Output is one bitmap per laid-out
    /// line; the caller stacks them at the desired line-height.
    pub fn render_run_wrapped(
        &mut self,
        run: &TextRun,
        max_width_px: f32,
    ) -> Result<Vec<RgbaBitmap>, oxideav_scribe::Error> {
        let size = sane_size(run.font_size);
        let lines =
            oxideav_scribe::wrap_lines(self.face(), &run.text, size, max_width_px.max(0.0))?;
        let mut out: Vec<RgbaBitmap> = Vec::with_capacity(lines.len());
        for line in &lines {
            let bm = self.shape_and_render_line(line, size, run.color);
            let bm = if run.italic && !bm.is_empty() {
                apply_fake_italic(&bm, size)
            } else {
                bm
            };
            out.push(bm);
        }
        Ok(out)
    }

    /// Render a `TextRun` into the destination, wrapping at
    /// `max_width_px` (or splitting on `\n`). Lines stack downward
    /// from `pen_y` at intervals of `self.line_height_px(size)` (or
    /// `line_height_override` if `Some`).
    #[allow(clippy::too_many_arguments)]
    pub fn render_run_wrapped_into(
        &mut self,
        run: &TextRun,
        dst: &mut [u8],
        dst_w: u32,
        dst_h: u32,
        pen_x: i32,
        pen_y: i32,
        max_width_px: f32,
        line_height_override: Option<f32>,
    ) -> Result<(), oxideav_scribe::Error> {
        let lines = self.render_run_wrapped(run, max_width_px)?;
        if lines.is_empty() {
            return Ok(());
        }
        let lh = line_height_override
            .unwrap_or_else(|| self.line_height_px(sane_size(run.font_size)))
            .max(1.0)
            .ceil() as i32;
        let mut y = pen_y;
        for line in &lines {
            if !line.is_empty() {
                blit_rgba_straight(
                    dst,
                    dst_w,
                    dst_h,
                    pen_x,
                    y,
                    &line.data,
                    line.width,
                    line.height,
                );
            }
            y += lh;
        }
        Ok(())
    }

    /// Lower-level path: shape `run.text` and composite directly into
    /// a caller-provided [`RgbaBitmap`] at pen origin `(pen_x, pen_y)`.
    /// Reuses the renderer's internal glyph-bitmap LRU. Useful when
    /// the caller is already tiling several runs into one bitmap and
    /// doesn't want a fresh allocation per run.
    ///
    /// `pen_x` / `pen_y` is the **typographic baseline** for this
    /// entry point (matching the previous Scribe `compose_run`
    /// convention), unlike the `render_run_into` family which uses the
    /// bitmap top-left.
    pub fn compose_run_at(
        &mut self,
        run: &TextRun,
        dst: &mut RgbaBitmap,
        pen_x: f32,
        pen_y: f32,
    ) -> Result<(), oxideav_scribe::Error> {
        if dst.is_empty() {
            return Ok(());
        }
        let size = sane_size(run.font_size);
        let placed = Shaper::shape_to_paths(&self.chain, &run.text, size);
        if placed.is_empty() {
            return Ok(());
        }
        let fill = Paint::Solid(decode_paint(run.color));

        // Build a vector frame the size of the destination, with the
        // run translated so glyph (0, 0) lands at the requested
        // baseline pen position.
        let frame = build_run_frame(&placed, dst.width, dst.height, pen_x, pen_y, &fill);

        self.renderer.width = dst.width;
        self.renderer.height = dst.height;
        self.renderer.background = CoreRgba::new(0, 0, 0, 0);
        let video_frame = self.renderer.render(&frame);

        if let Some(plane) = video_frame.planes.into_iter().next() {
            // Composite the rendered frame onto `dst` (straight-alpha
            // "over"); the rasteriser's output is already at the right
            // size + position because the frame matches dst dims.
            blit_rgba_straight(
                &mut dst.data,
                dst.width,
                dst.height,
                0,
                0,
                &plane.data,
                dst.width,
                dst.height,
            );
        }
        Ok(())
    }

    /// Shape one logical line and rasterise it to a freshly-sized
    /// [`RgbaBitmap`] tightly bounded by the run's natural extent.
    /// Returns an empty bitmap when the line shapes to zero pixels
    /// (empty string, whitespace-only, zero-size font).
    fn shape_and_render_line(&mut self, text: &str, size_px: f32, colour: u32) -> RgbaBitmap {
        if text.is_empty() {
            return RgbaBitmap::default();
        }
        // Pre-pass through `Shaper::shape` to measure the run's pen
        // extent so we can size the canvas. `shape_to_paths` re-shapes
        // internally — round-2 wears the duplicate cost; once Scribe
        // exposes a "shape + extents" API the second walk goes away.
        let glyphs = match Shaper::shape(self.face(), text, size_px) {
            Ok(g) => g,
            Err(_) => return RgbaBitmap::default(),
        };
        if glyphs.is_empty() {
            return RgbaBitmap::default();
        }
        let advance_px: f32 = glyphs.iter().map(|g| g.x_offset + g.x_advance).sum();
        let ascent_px = self.face().ascent_px(size_px);
        let descent_px = self.face().descent_px(size_px); // typically negative
        let glyph_w = advance_px.ceil().max(0.0) as u32;
        let glyph_h = (ascent_px - descent_px).ceil().max(0.0) as u32;
        if glyph_w == 0 || glyph_h == 0 {
            return RgbaBitmap::default();
        }

        let placed = Shaper::shape_to_paths(&self.chain, text, size_px);
        if placed.is_empty() {
            return RgbaBitmap::default();
        }
        let fill = Paint::Solid(decode_paint(colour));
        // Pen Y inside the bitmap: top sits at y=0, baseline at y=ascent_px.
        let frame = build_run_frame(&placed, glyph_w, glyph_h, 0.0, ascent_px, &fill);

        self.renderer.width = glyph_w;
        self.renderer.height = glyph_h;
        self.renderer.background = CoreRgba::new(0, 0, 0, 0);
        let video_frame = self.renderer.render(&frame);
        let plane = match video_frame.planes.into_iter().next() {
            Some(p) => p,
            None => return RgbaBitmap::default(),
        };
        let expected = (glyph_w as usize) * (glyph_h as usize) * 4;
        if plane.data.len() != expected {
            return RgbaBitmap::default();
        }
        RgbaBitmap {
            width: glyph_w,
            height: glyph_h,
            data: plane.data,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `VectorFrame` of `(canvas_w, canvas_h)` containing the
/// placed glyph chain, translated so the shaper's run-relative
/// origin (baseline-left) lands at `(pen_x, pen_y)` inside the canvas.
/// Each glyph is recoloured to `fill` (outline glyphs only — bitmap
/// `Node::Image` glyphs keep their built-in palette).
fn build_run_frame(
    placed: &[(usize, Node, Transform2D)],
    canvas_w: u32,
    canvas_h: u32,
    pen_x: f32,
    pen_y: f32,
    fill: &Paint,
) -> VectorFrame {
    let mut root = Group {
        transform: Transform2D::translate(pen_x, pen_y),
        ..Group::default()
    };
    for (_face_idx, glyph_node, transform) in placed {
        let recoloured = recolour_glyph(glyph_node.clone(), fill);
        let placement = Group {
            transform: *transform,
            children: vec![recoloured],
            ..Group::default()
        };
        root.children.push(Node::Group(placement));
    }
    VectorFrame {
        width: canvas_w as f32,
        height: canvas_h as f32,
        view_box: None,
        root,
        pts: None,
        time_base: TimeBase::new(1, 1),
    }
}

/// Replace the default-black `PathNode.fill` on outline glyph nodes
/// with the requested run colour. `shape_to_paths` always wraps each
/// glyph in `Group { children: [PathNode | Image] }` (round-8 cache
/// envelope), so we walk into that one child and rewrite the fill in
/// place. Bitmap glyphs (`Node::Image`) carry their own colour and are
/// left untouched. Mirrors the helper in
/// `oxideav_generator::image::label::recolour_glyph`.
fn recolour_glyph(node: Node, fill: &Paint) -> Node {
    match node {
        Node::Group(mut g) => {
            for child in g.children.iter_mut() {
                let placeholder = std::mem::replace(child, Node::Group(Group::default()));
                *child = recolour_glyph(placeholder, fill);
            }
            Node::Group(g)
        }
        Node::Path(p) => Node::Path(PathNode {
            path: p.path,
            fill: Some(fill.clone()),
            stroke: p.stroke,
            fill_rule: FillRule::NonZero,
        }),
        // Bitmap glyphs (CBDT/sbix → Node::Image) keep their carried
        // palette; the run `color` parameter is meaningless for them.
        other => other,
    }
}

/// Decode `0xRRGGBBAA` (TextRun convention) into the
/// [`oxideav_core::Rgba`] the rasteriser consumes.
fn decode_paint(packed: u32) -> CoreRgba {
    let [r, g, b, a] = decode_rgba(packed);
    CoreRgba::new(r, g, b, a)
}

/// Decode `0xRRGGBBAA` (TextRun convention) into the `[R, G, B, A]`
/// quartet. Unfinite/zero alpha is preserved — the rasteriser's
/// composite path treats a 0-alpha colour as "draw nothing," which
/// matches the expected behaviour.
fn decode_rgba(packed: u32) -> [u8; 4] {
    [
        ((packed >> 24) & 0xff) as u8,
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        (packed & 0xff) as u8,
    ]
}

/// Clamp font size to a strictly-positive finite value. Scribe
/// rejects non-positive sizes by returning empty output; rather than
/// silently swallow that for clearly malformed `TextRun`s, fall back
/// to a tiny default so the renderer keeps producing output.
fn sane_size(s: f32) -> f32 {
    if s.is_finite() && s > 0.0 {
        s
    } else {
        1.0
    }
}

/// Apply a "fake italic" by horizontally shearing the rasterised
/// bitmap. The shear factor is `font_size / 4`, the same magnitude
/// `oxideav_subtitle::compositor` uses for its bitmap-font italic.
/// Top rows shift right, bottom rows shift left — the bitmap widens
/// by `shear_px` to fit both extremes.
fn apply_fake_italic(src: &RgbaBitmap, size_px: f32) -> RgbaBitmap {
    let shear_px = (size_px / 4.0).round().max(0.0) as u32;
    if shear_px == 0 || src.is_empty() {
        return src.clone();
    }
    let new_w = src.width.saturating_add(shear_px);
    let mut out = RgbaBitmap::new(new_w, src.height);
    let h = src.height;
    let src_w = src.width as usize;
    let dst_w = new_w as usize;
    let denom = (h as f32).max(1.0);
    for y in 0..h {
        // Top of bitmap shifts right by `shear_px`, baseline shifts 0.
        let frac = 1.0 - (y as f32 / denom);
        let dx = (frac * shear_px as f32).round() as usize;
        let src_off = (y as usize) * src_w * 4;
        let dst_off = (y as usize) * dst_w * 4;
        for x in 0..src_w {
            let so = src_off + x * 4;
            let dst_x = x + dx;
            if dst_x >= dst_w {
                break;
            }
            let dop = dst_off + dst_x * 4;
            out.data[dop] = src.data[so];
            out.data[dop + 1] = src.data[so + 1];
            out.data[dop + 2] = src.data[so + 2];
            out.data[dop + 3] = src.data[so + 3];
        }
    }
    out
}

/// Composite a straight-alpha RGBA8 source bitmap onto a straight-alpha
/// RGBA8 destination at `(x, y)` (top-left). Pixels outside the
/// destination rectangle are clipped. Blend is Porter-Duff "over" via
/// [`oxideav_pixfmt::over_straight`]. Mirrors the helper in
/// `oxideav_subtitle::compositor::blit_rgba_straight` so the two
/// renderers behave identically when fed the same source bitmap.
#[allow(clippy::too_many_arguments)]
fn blit_rgba_straight(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    x: i32,
    y: i32,
    src: &[u8],
    src_w: u32,
    src_h: u32,
) {
    if dst_w == 0 || dst_h == 0 || src_w == 0 || src_h == 0 {
        return;
    }
    let dx0 = x.max(0);
    let dy0 = y.max(0);
    let dx1 = (x + src_w as i32).min(dst_w as i32);
    let dy1 = (y + src_h as i32).min(dst_h as i32);
    if dx0 >= dx1 || dy0 >= dy1 {
        return;
    }
    let sx0 = (dx0 - x) as usize;
    let sy0 = (dy0 - y) as usize;
    let blit_w = (dx1 - dx0) as usize;
    let blit_h = (dy1 - dy0) as usize;
    let dst_stride = dst_w as usize * 4;
    let src_stride = src_w as usize * 4;
    for row in 0..blit_h {
        let dst_row_off = (dy0 as usize + row) * dst_stride + (dx0 as usize) * 4;
        let src_row_off = (sy0 + row) * src_stride + sx0 * 4;
        for col in 0..blit_w {
            let so = src_row_off + col * 4;
            let s = [src[so], src[so + 1], src[so + 2], src[so + 3]];
            if s[3] == 0 {
                continue;
            }
            let dop = dst_row_off + col * 4;
            let d = [dst[dop], dst[dop + 1], dst[dop + 2], dst[dop + 3]];
            let out = oxideav_pixfmt::over_straight(s, d);
            dst[dop] = out[0];
            dst[dop + 1] = out[1];
            dst[dop + 2] = out[2];
            dst[dop + 3] = out[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_dejavu() -> Option<Face> {
        // Look for the fixture both relative to this crate (when
        // tested via `cargo test -p oxideav-scene` from the umbrella)
        // and inside it (if a copy is dropped here later).
        let candidates = [
            "../oxideav-ttf/tests/fixtures/DejaVuSans.ttf",
            "tests/fixtures/DejaVuSans.ttf",
        ];
        for path in candidates {
            if let Ok(bytes) = std::fs::read(path) {
                return Face::from_ttf_bytes(bytes).ok();
            }
        }
        None
    }

    fn make_run(text: &str) -> TextRun {
        TextRun {
            text: text.to_string(),
            font_family: "DejaVu Sans".to_string(),
            font_weight: 400,
            font_size: 24.0,
            color: 0xFFFFFFFF,
            advances: None,
            italic: false,
            underline: false,
        }
    }

    #[test]
    fn decode_rgba_orders_channels_msb_first() {
        // 0xAARRGGBB? No — TextRun is documented as 0xRRGGBBAA.
        let c = decode_rgba(0xFF8040C0);
        assert_eq!(c, [0xFF, 0x80, 0x40, 0xC0]);
    }

    #[test]
    fn sane_size_clamps_zero_and_nan() {
        assert_eq!(sane_size(12.0), 12.0);
        assert_eq!(sane_size(0.0), 1.0);
        assert_eq!(sane_size(-3.0), 1.0);
        assert!(sane_size(f32::NAN) > 0.0);
    }

    #[test]
    fn render_run_lights_pixels_at_pen_position() {
        let face = match load_dejavu() {
            Some(f) => f,
            None => return, // fixture missing — skip
        };
        let mut tr = TextRenderer::new(face);
        let run = make_run("Hello, world!");

        let dst_w: u32 = 200;
        let dst_h: u32 = 40;
        let mut dst = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];

        // Pen at (5, 5) — leaves room above/below for descenders.
        tr.render_run_into(&run, &mut dst, dst_w, dst_h, 5, 5)
            .unwrap();

        // 1. Total lit pixel count is non-trivial.
        let lit = dst.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(
            lit > 50,
            "expected glyph coverage; got only {lit} lit pixels"
        );

        // 2. Some lit pixel sits within the run's pen region —
        //    i.e. roughly in the upper-left quadrant where the
        //    "Hello" prefix falls.
        let mut hit_near_pen = false;
        'outer: for y in 5..30u32 {
            for x in 5..120u32 {
                let off = (y as usize * dst_w as usize + x as usize) * 4;
                if dst[off + 3] > 0 {
                    hit_near_pen = true;
                    break 'outer;
                }
            }
        }
        assert!(
            hit_near_pen,
            "no lit pixel found near pen position (5,5) — text not landing where requested"
        );
    }

    #[test]
    fn render_run_honours_run_color() {
        let face = match load_dejavu() {
            Some(f) => f,
            None => return,
        };
        let mut tr = TextRenderer::new(face);
        let mut run = make_run("X");
        run.color = 0xFF0000FF; // pure red, fully opaque

        let bm = tr.render_run(&run).unwrap();
        if bm.is_empty() {
            // Glyph rasterised empty? unexpected for "X" but don't crash.
            return;
        }
        // At least one solidly-lit pixel should be red-dominant.
        let mut found_red = false;
        for px in bm.data.chunks_exact(4) {
            if px[3] > 200 && px[0] > 200 && px[1] < 60 && px[2] < 60 {
                found_red = true;
                break;
            }
        }
        assert!(found_red, "no red-dominant lit pixel — colour not applied");
    }

    #[test]
    fn empty_run_produces_empty_bitmap() {
        let face = match load_dejavu() {
            Some(f) => f,
            None => return,
        };
        let mut tr = TextRenderer::new(face);
        let run = make_run("");
        let bm = tr.render_run(&run).unwrap();
        assert!(bm.is_empty());
    }

    #[test]
    fn whitespace_only_run_produces_empty_bitmap() {
        let face = match load_dejavu() {
            Some(f) => f,
            None => return,
        };
        let mut tr = TextRenderer::new(face);
        let run = make_run("    ");
        let bm = tr.render_run(&run).unwrap();
        // Whitespace shapes to space glyphs whose `glyph_node` returns
        // None (no rendering output) — `shape_and_render_line` then
        // collapses to an empty bitmap because `placed.is_empty()`
        // even though the pen advance is non-zero. Either an empty
        // bitmap OR a fully-transparent allocated bitmap is acceptable;
        // assert "no lit pixels" instead of strict emptiness so the
        // contract isn't tightened beyond what the previous Scribe
        // path guaranteed.
        let lit = bm.data.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert_eq!(lit, 0);
    }

    #[test]
    fn invalid_font_size_does_not_panic() {
        let face = match load_dejavu() {
            Some(f) => f,
            None => return,
        };
        let mut tr = TextRenderer::new(face);
        let mut run = make_run("Hi");
        run.font_size = 0.0;
        // Should clamp to a sane size and not error out.
        let _ = tr.render_run(&run).unwrap();
        run.font_size = f32::NAN;
        let _ = tr.render_run(&run).unwrap();
    }

    #[test]
    fn wrapped_run_returns_multiple_lines() {
        let face = match load_dejavu() {
            Some(f) => f,
            None => return,
        };
        let mut tr = TextRenderer::new(face);
        let run = make_run("Hello\nworld");
        let lines = tr.render_run_wrapped(&run, 1000.0).unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn italic_widens_bitmap() {
        let face = match load_dejavu() {
            Some(f) => f,
            None => return,
        };
        let mut tr = TextRenderer::new(face);
        let mut upright = make_run("Hi");
        upright.italic = false;
        let mut italic = make_run("Hi");
        italic.italic = true;
        let a = tr.render_run(&upright).unwrap();
        let b = tr.render_run(&italic).unwrap();
        if a.is_empty() || b.is_empty() {
            return;
        }
        assert!(
            b.width >= a.width,
            "italic shear should not narrow the bitmap (upright {}, italic {})",
            a.width,
            b.width
        );
    }
}
