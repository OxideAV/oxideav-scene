//! Renderer traits + a stub implementation.
//!
//! The real renderer will land as a separate crate (probably
//! `oxideav-scene-render`) and depend on the rasteriser + text
//! shaper. This crate defines the trait surface so downstream code
//! can write against it today.

use oxideav_core::{Error, Result};

use crate::duration::TimeStamp;
use crate::ops::ExportOp;
use crate::scene::Scene;

/// One output tick.
#[derive(Clone, Debug, Default)]
pub struct RenderedFrame {
    /// Optional reconstructed video frame. `None` for audio-only
    /// intervals or for exporters that don't emit raster.
    pub video: Option<oxideav_core::VideoFrame>,
    /// Audio samples for the interval since the previous
    /// `sample_at` call, at the scene's `sample_rate`. Interleaved
    /// f32. Always valid (silent if no cue fired).
    pub audio: Vec<f32>,
    /// Structured export ops (PDF runs, filter-format operators, …).
    pub operations: Vec<ExportOp>,
}

/// Driver trait — the outer render loop.
pub trait SceneRenderer {
    /// Prepare internal state for a fresh render. Called before the
    /// first `render_at` on a new scene or after `seek`.
    fn prepare(&mut self, scene: &Scene) -> Result<()>;

    /// Render the scene at timestamp `t`. `t` must be monotonically
    /// non-decreasing between consecutive calls unless `seek` is
    /// called in between.
    fn render_at(&mut self, scene: &Scene, t: TimeStamp) -> Result<RenderedFrame>;

    /// Jump to `t` — invalidates any per-object state the renderer
    /// cached from the previous position.
    fn seek(&mut self, t: TimeStamp) -> Result<()>;
}

/// Per-object sampler — one instance per `SceneObject`. Implementations
/// hold the object's decoder / glyph cache / bitmap handle and emit
/// the concrete pixel contribution for a given timestamp. Plug into a
/// [`SceneRenderer`] by registering a factory keyed on the object's
/// `ObjectKind` discriminant.
pub trait SceneSampler {
    /// Sample at time `t`. Returns `None` if the object has nothing
    /// to emit at this timestamp (e.g. a `Video` object whose next
    /// keyframe is still in the future).
    fn sample(&mut self, t: TimeStamp) -> Result<Option<SampleOutput>>;

    /// Natural size of the object before `Transform` is applied. Used
    /// for aspect-ratio-preserving layout in groups.
    fn natural_size(&self) -> (f32, f32);
}

/// What a [`SceneSampler`] hands back. Video frames go into the
/// compositor; text / raw ops flow through to the export pipeline.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum SampleOutput {
    Frame(oxideav_core::VideoFrame),
    Audio(Vec<f32>),
    /// Carry-through for vector / structured exports (PDF).
    StructuredOp(ExportOp),
}

/// Placeholder implementation. Every call returns
/// `Error::Unsupported` — included so downstream code can compile
/// against the trait today.
#[derive(Default)]
pub struct StubRenderer;

impl SceneRenderer for StubRenderer {
    fn prepare(&mut self, _scene: &Scene) -> Result<()> {
        Err(Error::unsupported(
            "oxideav-scene: renderer is a scaffold — real renderer lands as a separate crate",
        ))
    }

    fn render_at(&mut self, _scene: &Scene, _t: TimeStamp) -> Result<RenderedFrame> {
        Err(Error::unsupported(
            "oxideav-scene: renderer is a scaffold — real renderer lands as a separate crate",
        ))
    }

    fn seek(&mut self, _t: TimeStamp) -> Result<()> {
        Err(Error::unsupported(
            "oxideav-scene: renderer is a scaffold — real renderer lands as a separate crate",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_renderer_returns_unsupported() {
        let scene = Scene::default();
        let mut r = StubRenderer;
        let err = r.prepare(&scene).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));
        let err = r.render_at(&scene, 0).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));
    }
}
