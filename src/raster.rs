//! Vector → raster fallback for [`ObjectKind::Vector`] when the
//! output target is a raster pipeline.
//!
//! Only compiled with the default-on `raster` feature. Disable it
//! when the scene caller only targets vector outputs (PDF / SVG)
//! and doesn't want the rasteriser pulled in:
//!
//! ```toml
//! oxideav-scene = { version = "0.1", default-features = false }
//! ```
//!
//! For vector targets the writer (e.g. `oxideav-pdf`, `oxideav-svg`)
//! consumes the [`VectorFrame`] inside an
//! [`ObjectKind::Vector`](crate::ObjectKind::Vector) directly — no
//! rasterisation happens. This module only exists for the raster
//! path.
//!
//! The rasteriser's bitmap cache picks up `Group::cache_key`
//! (oxideav-core `04e662f`) automatically — scene callers don't
//! need to touch it.

use oxideav_core::{VectorFrame, VideoFrame};
use oxideav_raster::Renderer;

/// Rasterise `frame` to a `(width, height)` RGBA8 [`VideoFrame`]
/// suitable for compositing into a [`Canvas::Raster`](crate::Canvas)
/// pipeline. Wraps `oxideav_raster::Renderer::new(...).render(...)`
/// so callers don't need a direct dep on the rasteriser crate.
///
/// Width / height are output pixel dimensions — the caller's
/// canvas size, not the vector's intrinsic size. The rasteriser's
/// own viewport mapping handles the scale.
pub fn rasterize_vector(frame: &VectorFrame, width: u32, height: u32) -> VideoFrame {
    Renderer::new(width, height).render(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::{Group, TimeBase};

    fn empty_frame(w: f32, h: f32) -> VectorFrame {
        VectorFrame {
            width: w,
            height: h,
            view_box: None,
            root: Group::default(),
            pts: None,
            time_base: TimeBase::new(1, 1),
        }
    }

    #[test]
    fn rasterize_empty_returns_blank_frame() {
        let vf = empty_frame(64.0, 48.0);
        let out = rasterize_vector(&vf, 64, 48);
        // Empty vector → renderer returns a fully-transparent
        // RGBA8 buffer at the requested dims. We assert the buffer
        // is the right size (64 * 48 * 4 = 12288 bytes) on plane
        // 0; pixel content is the rasteriser's contract, not
        // ours.
        assert!(!out.planes.is_empty());
        assert_eq!(out.planes[0].data.len(), 64 * 48 * 4);
    }
}
