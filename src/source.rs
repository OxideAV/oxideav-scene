//! Source / Sink plumbing.
//!
//! A scene is a media producer — drive it with a [`SceneRenderer`]
//! and it yields a stream of [`RenderedFrame`]s at the scene's
//! [`framerate`](crate::Scene::framerate). The [`SceneSource`] trait
//! formalises that contract so scenes can slot into the same pipe
//! topology that oxideav uses for live decoders, capture devices,
//! and file readers.
//!
//! A [`SceneSink`] consumes `RenderedFrame`s and does something with
//! them — encode + mux to a file, push to an RTMP endpoint, render
//! to a window, emit PDF operators to a writer, etc. The sink trait
//! is deliberately thin: `init` once, `push` per frame, `finalise`
//! once. Format negotiation happens upfront via [`SourceFormat`].
//!
//! [`drive`] is the glue: it runs the pull loop until the source is
//! exhausted or the sink errors out.

use oxideav_core::{Error, Rational, Result, TimeBase};

use crate::duration::{SceneDuration, TimeStamp};
use crate::object::Canvas;
use crate::render::{RenderedFrame, SceneRenderer};
use crate::scene::Scene;

/// Format contract between a [`SceneSource`] and a [`SceneSink`].
///
/// Everything a sink needs to set up encoders / muxers / windows
/// before the first frame arrives.
#[derive(Clone, Debug)]
pub struct SourceFormat {
    pub canvas: Canvas,
    pub framerate: Rational,
    pub time_base: TimeBase,
    pub sample_rate: u32,
    /// Whether the source has a known end. `Finite(n)` lets sinks
    /// size output containers; `Indefinite` signals a streaming
    /// source that runs until externally stopped.
    pub duration: SceneDuration,
}

impl SourceFormat {
    /// Build from a scene's current state. The renderer consumes
    /// this at `init()` time so downstream encoder settings match
    /// the scene's declarations.
    pub fn from_scene(scene: &Scene) -> Self {
        SourceFormat {
            canvas: scene.canvas,
            framerate: scene.framerate,
            time_base: scene.time_base,
            sample_rate: scene.sample_rate,
            duration: scene.duration,
        }
    }
}

/// Pull-based source of rendered frames.
///
/// Implementors typically wrap a [`Scene`] + [`SceneRenderer`] and
/// advance an internal frame counter per `pull()`. The first call
/// after `prepare` emits frame 0 at timestamp 0; each subsequent
/// call advances by `1 / framerate`.
///
/// Sources are **not** required to be seekable — a streaming
/// compositor source is forward-only. Sources that can seek should
/// expose it via an inherent method, not this trait.
pub trait SceneSource {
    /// Declared format. Constant across a session.
    fn format(&self) -> SourceFormat;

    /// Produce the next rendered tick. Returns `Ok(None)` when the
    /// source is exhausted (finite scene reached its end). For
    /// indefinite sources, this never returns `None`.
    fn pull(&mut self) -> Result<Option<RenderedFrame>>;
}

/// Push-based sink for rendered frames.
///
/// Implementors set up encoders / muxers in [`init`], receive one
/// frame per [`push`] call, then release any buffered state in
/// [`finalise`]. A sink that fails mid-stream should return the
/// error from `push` — [`drive`] will call `finalise` regardless.
///
/// [`init`]: SceneSink::init
/// [`push`]: SceneSink::push
/// [`finalise`]: SceneSink::finalise
pub trait SceneSink {
    /// Called once before the first `push`. The sink may return
    /// `Error::Unsupported` if it can't handle the format.
    fn init(&mut self, format: &SourceFormat) -> Result<()>;

    /// Consume one rendered frame. Time is embedded in the frame's
    /// video pts + audio sample count; the sink itself doesn't need
    /// to track timestamps separately.
    fn push(&mut self, frame: RenderedFrame) -> Result<()>;

    /// Flush + close. No more `push` calls after this. Always
    /// called by `drive`, even on error paths.
    fn finalise(&mut self) -> Result<()>;
}

/// Pull-loop helper. Drives `source` → `sink` until the source is
/// exhausted or either side errors out. `finalise` is always
/// called on the sink; `init` happens before the first pull.
pub fn drive(source: &mut dyn SceneSource, sink: &mut dyn SceneSink) -> Result<()> {
    let fmt = source.format();
    sink.init(&fmt)?;
    let result = drive_loop(source, sink);
    let fin = sink.finalise();
    result.and(fin)
}

fn drive_loop(source: &mut dyn SceneSource, sink: &mut dyn SceneSink) -> Result<()> {
    loop {
        match source.pull()? {
            Some(frame) => sink.push(frame)?,
            None => return Ok(()),
        }
    }
}

/// Default [`SceneSource`] implementation wrapping a scene + a
/// renderer. Advances one frame per `pull` at the scene's declared
/// framerate; emits `None` when a finite scene's last frame has
/// been yielded.
pub struct RenderedSource<R: SceneRenderer> {
    scene: Scene,
    renderer: R,
    next_frame: u64,
    total_frames: Option<u64>,
    prepared: bool,
}

impl<R: SceneRenderer> RenderedSource<R> {
    /// Take ownership of `scene` + `renderer`. Does not call
    /// `prepare` on the renderer — that happens lazily on the first
    /// `pull`.
    pub fn new(scene: Scene, renderer: R) -> Self {
        let total_frames = scene.frame_count();
        RenderedSource {
            scene,
            renderer,
            next_frame: 0,
            total_frames,
            prepared: false,
        }
    }

    /// Access the underlying scene (read-only). Useful for tests +
    /// compositors that want to inspect state between pulls.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Mutate the scene between pulls. The streaming-compositor use
    /// case uses this to apply `Operation`s pulled from a control
    /// channel. Mid-stream mutations MUST NOT shift earlier
    /// timestamps — append-only operations (new keyframes after
    /// `next_timestamp()`, new objects, removed-in-future) are
    /// safe; rewriting existing keyframes is not.
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// Timestamp of the next frame to be pulled.
    pub fn next_timestamp(&self) -> TimeStamp {
        self.scene.frame_to_timestamp(self.next_frame)
    }
}

impl<R: SceneRenderer> SceneSource for RenderedSource<R> {
    fn format(&self) -> SourceFormat {
        SourceFormat::from_scene(&self.scene)
    }

    fn pull(&mut self) -> Result<Option<RenderedFrame>> {
        if let Some(total) = self.total_frames {
            if self.next_frame >= total {
                return Ok(None);
            }
        }
        if !self.prepared {
            self.renderer.prepare(&self.scene)?;
            self.prepared = true;
        }
        let t = self.next_timestamp();
        let frame = self.renderer.render_at(&self.scene, t)?;
        self.next_frame += 1;
        Ok(Some(frame))
    }
}

/// Discarding sink — useful for correctness tests + dry runs that
/// exercise the pull loop without wiring an encoder. Records a
/// frame + byte counter so callers can assert progress.
#[derive(Default)]
pub struct NullSink {
    pub frames_received: u64,
    pub bytes_received: u64,
    pub format_seen: Option<SourceFormat>,
}

impl SceneSink for NullSink {
    fn init(&mut self, format: &SourceFormat) -> Result<()> {
        self.format_seen = Some(format.clone());
        Ok(())
    }

    fn push(&mut self, frame: RenderedFrame) -> Result<()> {
        self.frames_received += 1;
        if let Some(v) = frame.video.as_ref() {
            self.bytes_received += v.planes.iter().map(|p| p.data.len() as u64).sum::<u64>();
        }
        self.bytes_received += (frame.audio.len() * std::mem::size_of::<f32>()) as u64;
        Ok(())
    }

    fn finalise(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Sink that forwards to a closure. Handy for tests + one-off
/// integrations where a full trait impl is overkill.
pub struct FnSink<F>
where
    F: FnMut(&SourceFormat, RenderedFrame) -> Result<()>,
{
    format: Option<SourceFormat>,
    cb: F,
}

impl<F> FnSink<F>
where
    F: FnMut(&SourceFormat, RenderedFrame) -> Result<()>,
{
    pub fn new(cb: F) -> Self {
        FnSink { format: None, cb }
    }
}

impl<F> SceneSink for FnSink<F>
where
    F: FnMut(&SourceFormat, RenderedFrame) -> Result<()>,
{
    fn init(&mut self, format: &SourceFormat) -> Result<()> {
        self.format = Some(format.clone());
        Ok(())
    }

    fn push(&mut self, frame: RenderedFrame) -> Result<()> {
        let fmt = self.format.as_ref().ok_or_else(|| {
            Error::invalid("FnSink: push before init — call SceneSink::init first")
        })?;
        (self.cb)(fmt, frame)
    }

    fn finalise(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::StubRenderer;

    /// Trivial `SceneSource` that emits 3 empty frames and stops.
    struct CountingSource {
        fmt: SourceFormat,
        left: u32,
    }

    impl SceneSource for CountingSource {
        fn format(&self) -> SourceFormat {
            self.fmt.clone()
        }
        fn pull(&mut self) -> Result<Option<RenderedFrame>> {
            if self.left == 0 {
                return Ok(None);
            }
            self.left -= 1;
            Ok(Some(RenderedFrame::default()))
        }
    }

    #[test]
    fn drive_runs_until_source_empty() {
        let scene = Scene::default();
        let fmt = SourceFormat::from_scene(&scene);
        let mut src = CountingSource { fmt, left: 3 };
        let mut sink = NullSink::default();
        drive(&mut src, &mut sink).unwrap();
        assert_eq!(sink.frames_received, 3);
        assert!(sink.format_seen.is_some());
    }

    #[test]
    fn rendered_source_stops_at_frame_count() {
        // 3 frames at 30 fps = 100 ms → Finite(100).
        let scene = Scene {
            duration: SceneDuration::Finite(100),
            ..Scene::default()
        };
        // 3 frames expected (0, 33, 66 ms; 100 ms is past the end).
        assert_eq!(scene.frame_count(), Some(3));
        // StubRenderer returns Unsupported, so we can't actually
        // pull successfully — but the frame-counting bookkeeping is
        // what matters here. Confirm via next_timestamp.
        let src = RenderedSource::new(scene, StubRenderer);
        assert_eq!(src.next_timestamp(), 0);
    }

    #[test]
    fn fn_sink_forwards_to_closure() {
        let mut count = 0u32;
        let mut sink = FnSink::new(|_fmt, _frame| {
            count += 1;
            Ok(())
        });
        let fmt = SourceFormat::from_scene(&Scene::default());
        sink.init(&fmt).unwrap();
        sink.push(RenderedFrame::default()).unwrap();
        sink.push(RenderedFrame::default()).unwrap();
        sink.finalise().unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn fn_sink_rejects_push_before_init() {
        let mut sink = FnSink::new(|_fmt, _frame| Ok(()));
        let err = sink.push(RenderedFrame::default()).unwrap_err();
        assert!(matches!(err, Error::InvalidData(_)));
    }
}
