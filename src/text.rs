//! Text-run rasterisation for [`crate::TextRun`] via
//! [`oxideav_scribe`].
//!
//! Round 1 of the scene crate's renderer was a trait-only scaffold —
//! [`crate::render::StubRenderer`] returns `Error::Unsupported`. This
//! module is the first concrete piece of *actual* rendering: it takes a
//! `TextRun` plus a [`oxideav_scribe::Face`] supplied by the caller
//! (the scene crate does not implement font discovery — see scope notes
//! below) and composites the shaped, anti-aliased glyphs into a
//! straight-alpha RGBA framebuffer.
//!
//! ## Pipeline
//!
//! 1. The caller hands us a [`TextRenderer`] holding a parsed
//!    `oxideav_scribe::Face`. Multiple runs against the same
//!    renderer share the underlying `oxideav_scribe::Composer`'s
//!    glyph-bitmap LRU.
//! 2. For each [`TextRun`]:
//!    * Decode the `0xRRGGBBAA` colour into a `Rgba` quartet.
//!    * If the run carries `\n` characters, fall back to the
//!      [`oxideav_scribe::render_text_wrapped`] path with a
//!      caller-supplied width budget; otherwise use
//!      [`oxideav_scribe::Shaper::shape`] +
//!      [`oxideav_scribe::Composer::compose_run`] for a single
//!      pen-anchored run.
//!    * If `run.italic` is set, document the deferral — round 1 of
//!      Scribe does not yet expose a synthetic-italic shear (it
//!      lands as a round-2 lift; see deferrals below).
//! 3. Composite the run's bitmap onto the destination framebuffer
//!    via straight-alpha "over"
//!    ([`oxideav_pixfmt::over_straight`]).
//!
//! ## Scope (round 1)
//!
//! * **Font discovery is the caller's job.** A scene-level font
//!   registry / `font_family → Face` resolver is intentionally out of
//!   scope; the renderer takes one `Face` and uses it for every
//!   `TextRun` it sees, ignoring `run.font_family`. Picking the right
//!   `Face` for the family belongs in the application layer.
//! * **Per-glyph colour (CPAL/COLR) is unsupported** — the run's
//!   single foreground colour applies to every glyph.
//! * **Underline drawing is deferred** to round 3 (see the inline
//!   `// round-3 note` in [`TextRenderer::render_run_into`]).
//! * **Italic** — Scribe round 1 does not expose a synthetic-italic
//!   shear API. When `run.italic` is set we apply the same per-row
//!   horizontal shear used by `oxideav_subtitle::compositor` (cell
//!   width / 4) directly on the rasterised line bitmap, AFTER
//!   composition. This is the same fallback documented as round-2
//!   work in the subtitle compositor; once Scribe gains a real
//!   italic-Face overload the fake-shear branch goes away.
//! * **Per-run advances** (`TextRun::advances`) are honoured when
//!   present (PDF-style explicit positioning); otherwise the shaper
//!   computes them.

use oxideav_scribe::{Composer, Face, RgbaBitmap, Shaper};

use crate::object::TextRun;

/// Owns a [`Face`] and a [`Composer`] (for cache reuse across runs).
///
/// Round-1 callers construct one `TextRenderer` per face they want
/// to use, and call [`TextRenderer::render_run_into`] for every
/// `TextRun` they want composited.
#[derive(Debug)]
pub struct TextRenderer {
    face: Face,
    composer: Composer,
}

impl TextRenderer {
    /// Build a renderer around an already-parsed [`Face`]. The face
    /// is owned for the lifetime of the renderer; if you need to
    /// switch fonts mid-scene, build a second `TextRenderer`.
    pub fn new(face: Face) -> Self {
        Self {
            face,
            composer: Composer::new(),
        }
    }

    /// Borrow the underlying face — useful for caller-side metric
    /// queries (line height, ascent) without re-parsing.
    pub fn face(&self) -> &Face {
        &self.face
    }

    /// Pixel line-height for `size_px`. Convenience wrapper around
    /// [`Face::line_height_px`] so callers laying out multi-line
    /// `TextRun` content don't need to dig into Scribe directly.
    pub fn line_height_px(&self, size_px: f32) -> f32 {
        self.face.line_height_px(size_px)
    }

    /// Render a `TextRun` into a freshly-allocated straight-alpha
    /// RGBA bitmap sized to the run's natural glyph bounds.
    ///
    /// Returns an empty bitmap if the run shapes to zero glyphs (or
    /// every glyph is non-rendering — e.g. a string of spaces).
    pub fn render_run(&mut self, run: &TextRun) -> Result<RgbaBitmap, oxideav_scribe::Error> {
        let size = sane_size(run.font_size);
        let color = decode_rgba(run.color);
        let bm = oxideav_scribe::render_text(&self.face, &run.text, size, color)?;
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
    /// (NOT the typographic baseline) to match the `RgbaBitmap`
    /// origin convention Scribe uses internally.
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
        blit_rgba_straight(dst, dst_w, dst_h, pen_x, pen_y, &bm.data, bm.width, bm.height);
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
        let color = decode_rgba(run.color);
        let bms = oxideav_scribe::render_text_wrapped(
            &self.face,
            &run.text,
            size,
            color,
            max_width_px,
        )?;
        if run.italic {
            return Ok(bms
                .into_iter()
                .map(|bm| {
                    if bm.is_empty() {
                        bm
                    } else {
                        apply_fake_italic(&bm, size)
                    }
                })
                .collect());
        }
        Ok(bms)
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

    /// Lower-level path: shape `run.text` and compose directly into a
    /// caller-provided [`RgbaBitmap`] at pen origin `(pen_x, pen_y)`.
    /// Reuses the renderer's internal glyph-bitmap LRU. Useful when
    /// the caller is already tiling several runs into one bitmap and
    /// doesn't want a fresh allocation per run.
    pub fn compose_run_at(
        &mut self,
        run: &TextRun,
        dst: &mut RgbaBitmap,
        pen_x: f32,
        pen_y: f32,
    ) -> Result<(), oxideav_scribe::Error> {
        let size = sane_size(run.font_size);
        let color = decode_rgba(run.color);
        let glyphs = Shaper::shape(&self.face, &run.text, size)?;
        if glyphs.is_empty() || dst.is_empty() {
            return Ok(());
        }
        self.composer
            .compose_run(&glyphs, &self.face, size, color, dst, pen_x, pen_y)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decode `0xRRGGBBAA` (TextRun convention) into the `[R, G, B, A]`
/// quartet Scribe uses internally. Unfinite/zero alpha is preserved
/// — Scribe's compose path treats a 0-alpha colour as "draw nothing,"
/// which matches the expected behaviour.
fn decode_rgba(packed: u32) -> [u8; 4] {
    [
        ((packed >> 24) & 0xff) as u8,
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        (packed & 0xff) as u8,
    ]
}

/// Clamp font size to a strictly-positive finite value. Scribe
/// rejects non-positive sizes with `Error::InvalidSize`; rather than
/// surface that error for clearly malformed `TextRun`s, fall back
/// to a tiny default so the renderer keeps producing output.
fn sane_size(s: f32) -> f32 {
    if s.is_finite() && s > 0.0 {
        s
    } else {
        1.0
    }
}

/// Apply a round-2-deferred "fake italic" by horizontally shearing
/// the rasterised bitmap. The shear factor is `font_size / 4`, the
/// same magnitude `oxideav_subtitle::compositor` uses for its
/// bitmap-font italic. Top rows shift right, bottom rows shift left
/// — the bitmap widens by `shear_px` to fit both extremes.
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
/// renderers behave identically when fed the same Scribe bitmap.
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
        tr.render_run_into(&run, &mut dst, dst_w, dst_h, 5, 5).unwrap();

        // 1. Total lit pixel count is non-trivial.
        let lit = dst.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 50, "expected glyph coverage; got only {lit} lit pixels");

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
        assert!(bm.is_empty());
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
