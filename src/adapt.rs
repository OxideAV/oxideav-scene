//! Automatic pixel-format adaptation for scene I/O.
//!
//! A scene's [`Canvas`] declares the composition's pixel format. Two
//! places need conversion:
//!
//! 1. **Inbound** — when a video / image / live source feeds into a
//!    scene object. Source frames can be any pixel format (YUV420P
//!    from an H.264 decoder, BGRA from a capture card, RGB24 from a
//!    PNG, …); the renderer converts them to the canvas format
//!    before compositing. Use [`adapt_frame_to_canvas`] for that.
//!
//! 2. **Outbound** — when a [`SceneSink`] expects a different format
//!    than the scene produces. Wrap the source in
//!    [`AdaptedSource`] — it intercepts each `pull()`, converts the
//!    rendered frame to the sink's target format, and updates the
//!    reported [`SourceFormat`] so `init()` tells the sink the
//!    right thing.
//!
//! Both paths delegate to [`oxideav_pixfmt::convert`]. Canvases that
//! don't declare a raster pixel format (e.g. [`Canvas::Vector`] for
//! PDF pages) pass frames through unchanged — vector exports don't
//! go through a raster conversion step.

use oxideav_core::{PixelFormat, Result, VideoFrame};
use oxideav_pixfmt::ConvertOptions;

use crate::object::Canvas;
use crate::render::RenderedFrame;
use crate::source::{SceneSource, SourceFormat};

/// Convert `frame` to `target`. No-op when formats already match.
pub fn adapt_frame_to(frame: VideoFrame, target: PixelFormat) -> Result<VideoFrame> {
    if frame.format == target {
        return Ok(frame);
    }
    oxideav_pixfmt::convert(&frame, target, &ConvertOptions::default())
}

/// Convert `frame` so it matches the canvas pixel format. For
/// vector canvases (which don't rasterise) the frame passes through.
pub fn adapt_frame_to_canvas(frame: VideoFrame, canvas: &Canvas) -> Result<VideoFrame> {
    match canvas {
        Canvas::Raster { pixel_format, .. } => adapt_frame_to(frame, *pixel_format),
        Canvas::Vector { .. } => Ok(frame),
    }
}

/// Source wrapper that converts every emitted frame to a target
/// pixel format.
///
/// Overrides the reported [`SourceFormat`] so the downstream sink's
/// `init()` sees the adapted canvas, not the scene's native one.
/// Cheap when the formats already match (the adapter short-circuits
/// in [`adapt_frame_to`]).
pub struct AdaptedSource<S: SceneSource> {
    inner: S,
    target: PixelFormat,
}

impl<S: SceneSource> AdaptedSource<S> {
    /// Wrap `inner`, converting every pulled frame to `target`. Use
    /// this when a sink accepts a specific pixel format that differs
    /// from the scene's canvas (e.g. RGB24 for a JPEG writer while
    /// the scene composes in YUV420P).
    pub fn new(inner: S, target: PixelFormat) -> Self {
        AdaptedSource { inner, target }
    }

    /// Access the wrapped source.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Mutable access to the wrapped source — useful for the
    /// streaming-compositor pattern where the caller mutates scene
    /// state between pulls.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S: SceneSource> SceneSource for AdaptedSource<S> {
    fn format(&self) -> SourceFormat {
        let mut f = self.inner.format();
        // Swap the pixel format inside a Raster canvas. Vector
        // canvases pass through — they don't declare one.
        if let Canvas::Raster {
            ref mut pixel_format,
            ..
        } = f.canvas
        {
            *pixel_format = self.target;
        }
        f
    }

    fn pull(&mut self) -> Result<Option<RenderedFrame>> {
        let Some(mut frame) = self.inner.pull()? else {
            return Ok(None);
        };
        if let Some(video) = frame.video.take() {
            frame.video = Some(adapt_frame_to(video, self.target)?);
        }
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Scene;
    use crate::source::SceneSource;
    use oxideav_core::{Rational, TimeBase, VideoFrame, VideoPlane};

    fn yuv420p_frame(width: u32, height: u32) -> VideoFrame {
        let y_size = (width * height) as usize;
        let c_size = ((width / 2) * (height / 2)) as usize;
        VideoFrame {
            format: PixelFormat::Yuv420P,
            width,
            height,
            pts: None,
            time_base: TimeBase::new(1, 30),
            planes: vec![
                VideoPlane {
                    stride: width as usize,
                    data: vec![128; y_size],
                },
                VideoPlane {
                    stride: (width / 2) as usize,
                    data: vec![128; c_size],
                },
                VideoPlane {
                    stride: (width / 2) as usize,
                    data: vec![128; c_size],
                },
            ],
        }
    }

    #[test]
    fn adapt_to_same_format_is_identity() {
        let f = yuv420p_frame(8, 8);
        let out = adapt_frame_to(f.clone(), PixelFormat::Yuv420P).unwrap();
        assert_eq!(out.format, PixelFormat::Yuv420P);
        assert_eq!(out.planes[0].data, f.planes[0].data);
    }

    #[test]
    fn adapt_to_canvas_vector_passes_through() {
        let f = yuv420p_frame(8, 8);
        let canvas = Canvas::Vector {
            width: 595.0,
            height: 842.0,
            unit: crate::object::LengthUnit::Point,
        };
        let out = adapt_frame_to_canvas(f, &canvas).unwrap();
        assert_eq!(out.format, PixelFormat::Yuv420P);
    }

    struct StaticSource {
        fmt: SourceFormat,
        frames_left: u32,
    }

    impl SceneSource for StaticSource {
        fn format(&self) -> SourceFormat {
            self.fmt.clone()
        }
        fn pull(&mut self) -> Result<Option<RenderedFrame>> {
            if self.frames_left == 0 {
                return Ok(None);
            }
            self.frames_left -= 1;
            Ok(Some(RenderedFrame {
                video: Some(yuv420p_frame(8, 8)),
                audio: Vec::new(),
                operations: Vec::new(),
            }))
        }
    }

    #[test]
    fn adapted_source_reports_target_format() {
        let scene = Scene {
            framerate: Rational::new(30, 1),
            ..Scene::default()
        };
        let inner = StaticSource {
            fmt: SourceFormat::from_scene(&scene),
            frames_left: 1,
        };
        let adapted = AdaptedSource::new(inner, PixelFormat::Rgba);
        match adapted.format().canvas {
            Canvas::Raster { pixel_format, .. } => assert_eq!(pixel_format, PixelFormat::Rgba),
            _ => panic!("expected Raster"),
        }
    }

    #[test]
    fn adapted_source_converts_on_pull() {
        // Yuv420P → Rgba is a supported pair in oxideav-pixfmt; the
        // conversion just needs to produce a frame whose `format`
        // field is now Rgba.
        let scene = Scene::default();
        let inner = StaticSource {
            fmt: SourceFormat::from_scene(&scene),
            frames_left: 1,
        };
        let mut adapted = AdaptedSource::new(inner, PixelFormat::Rgba);
        let out = adapted.pull().unwrap().expect("frame");
        let video = out.video.unwrap();
        assert_eq!(video.format, PixelFormat::Rgba);
        assert_eq!(video.width, 8);
        assert_eq!(video.height, 8);
    }
}
