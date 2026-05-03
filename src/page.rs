//! Page-based timing model.
//!
//! The default scene timing — [`SceneDuration::Finite`] /
//! [`SceneDuration::Indefinite`] — is **timeline-based**: every event
//! lives on a continuous millisecond (or `time_base`-tick) axis, and
//! the renderer samples that axis at the scene's framerate.
//!
//! That model is the right shape for the streaming compositor and
//! the NLE timeline, but it's the wrong shape for paged content
//! (PDF, multi-page TIFF, EPUB-style flowed documents). A PDF is not
//! "the page-1 layout from t=0 to t=33ms, the page-2 layout from
//! t=33ms to t=66ms"; a PDF is a sequence of independently-sized
//! page surfaces with no temporal relationship between them.
//!
//! [`Page`] models that. A [`Scene`] carries a `pages: Option<Vec<Page>>`
//! field — `Some(...)` for paged content, `None` for time-based.
//! The two are mutually exclusive at render dispatch:
//!
//! - PDF / multi-page TIFF writers accept only `Some(pages)`.
//! - PNG / MP4 / video writers accept only `None` (timeline mode).
//! - SVG is single-page; it accepts either (single page → render
//!   that page, timeline → render the frame at `t=0`).
//!
//! [`Scene::pages_to_timeline`] / [`Scene::timeline_to_pages`] adapt
//! between the two when the consumer needs to bridge.
//!
//! [`SceneDuration::Finite`]: crate::duration::SceneDuration::Finite
//! [`SceneDuration::Indefinite`]: crate::duration::SceneDuration::Indefinite
//! [`Scene`]: crate::scene::Scene
//! [`Scene::pages_to_timeline`]: crate::scene::Scene::pages_to_timeline
//! [`Scene::timeline_to_pages`]: crate::scene::Scene::timeline_to_pages

use oxideav_core::{Group, TimeBase, VectorFrame};

/// One page of paged content.
///
/// The content is an [`oxideav_core::VectorFrame`] — every page is
/// a self-contained vector composition. This is the natural shape
/// for PDF (a PDF page IS a vector composition), and rasterisable
/// via `oxideav_raster::Renderer` for raster-target writers (page
/// preview thumbnails, `pages_to_timeline()` adaptation).
///
/// Width / height are per-page because PDF, multi-page TIFF, and
/// EPUB all support varying page sizes within a single document
/// (cover spread + body pages, mixed portrait / landscape, etc).
/// Units match the scene's [`Canvas::Vector`] unit if the canvas
/// is vector; for raster canvases the values are in canvas pixels.
///
/// [`Canvas::Vector`]: crate::object::Canvas::Vector
#[derive(Clone, Debug)]
pub struct Page {
    /// Page width in the scene canvas's unit (PDF points, mm, …).
    pub width: f32,
    /// Page height in the same unit as [`width`](Self::width).
    pub height: f32,
    /// Vector content for this page.
    pub content: VectorFrame,
    /// Optional human-readable page label (PDF `/Info` page labels:
    /// "iv", "12-A", "Contents"). Distinct from the 1-based page
    /// index, which is implicit from the vector's position.
    pub label: Option<String>,
    /// Page rotation in degrees clockwise. Only `0`, `90`, `180`,
    /// and `270` are meaningful — values outside that set are
    /// renderer-defined. Matches PDF `/Rotate` semantics.
    pub orientation: u16,
}

/// Build an empty [`VectorFrame`] sized to `(width, height)` in the
/// page's own coordinate system. Used by both [`Page::default`] and
/// [`Page::new`].
fn empty_vector_frame(width: f32, height: f32) -> VectorFrame {
    VectorFrame {
        width,
        height,
        view_box: None,
        root: Group::default(),
        pts: None,
        time_base: TimeBase::new(1, 1),
    }
}

impl Default for Page {
    fn default() -> Self {
        // A4 in PDF points (1/72 inch): 595 x 842.
        let (w, h) = (595.0, 842.0);
        Page {
            width: w,
            height: h,
            content: empty_vector_frame(w, h),
            label: None,
            orientation: 0,
        }
    }
}

impl Page {
    /// Build a page with the given dims + empty content. Width /
    /// height are in the scene canvas's unit.
    pub fn new(width: f32, height: f32) -> Self {
        Page {
            width,
            height,
            content: empty_vector_frame(width, height),
            label: None,
            orientation: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_default_is_a4_portrait() {
        let p = Page::default();
        assert_eq!(p.width, 595.0);
        assert_eq!(p.height, 842.0);
        assert_eq!(p.orientation, 0);
        assert!(p.label.is_none());
    }

    #[test]
    fn page_new_sets_dims() {
        let p = Page::new(612.0, 792.0); // US Letter
        assert_eq!(p.width, 612.0);
        assert_eq!(p.height, 792.0);
    }
}
