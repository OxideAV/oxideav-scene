//! A time-based composition model for oxideav.
//!
//! The type surface (scene, objects, animations, time-base) is in
//! place, and concrete per-object rendering lands here piecemeal.
//! [`StubRenderer`] remains as the always-`Error::Unsupported`
//! placeholder for downstream code that only needs the trait shape.
//!
//! Two concrete renderers exist:
//!
//! - [`text::TextRenderer`] composites a single [`TextRun`] onto a
//!   straight-alpha RGBA framebuffer via [`oxideav_scribe`] (TrueType
//!   shaping + scanline rasterisation). The caller supplies the
//!   [`oxideav_scribe::Face`]; scene-level font discovery is out of
//!   scope.
//! - [`raster_renderer::RasterRenderer`] implements the full
//!   [`SceneRenderer`] driver: it walks [`Scene::sampled_at`] in paint
//!   order and composites the *vector* slice of a scene — backgrounds,
//!   [`object::Shape`]s, and [`ObjectKind::Vector`] objects — into an
//!   RGBA8 [`oxideav_core::VideoFrame`] through
//!   [`oxideav_raster::Renderer`], honouring per-object transform,
//!   opacity, and clip. Resource-backed kinds (image / video / live /
//!   text / group) are skipped pending a font-registry / decoder-aware
//!   renderer.
//!
//! See [`README.md`](../README.md) for the full design + the three
//! target use cases (PDF pages, RTMP streaming compositor, NLE
//! timeline).
//!
//! # Quick tour
//!
//! ```no_run
//! use oxideav_scene::{Scene, Canvas, SceneDuration, SceneObject};
//! use oxideav_core::TimeBase;
//!
//! let scene = Scene {
//!     canvas: Canvas::raster(1920, 1080),
//!     duration: SceneDuration::Finite(30_000),
//!     time_base: TimeBase::new(1, 1_000),
//!     sample_rate: 48_000,
//!     ..Scene::default()
//! };
//! assert_eq!(scene.canvas.raster_size(), Some((1920, 1080)));
//! ```
//!
//! The hierarchy:
//!
//! - [`Scene`] — root container. Has a [`Canvas`], a [`SceneDuration`],
//!   and a vector of [`SceneObject`]s.
//! - [`SceneObject`] — one element on the scene. Carries a
//!   [`Transform`], a [`Lifetime`], a list of [`Animation`]s, a blend
//!   mode + effects chain, and an [`ObjectKind`] payload.
//! - [`Animation`] — a per-property keyframe track. Each
//!   [`Keyframe`] pins a value at a point in time; consecutive
//!   keyframes interpolate via [`Easing`].
//! - [`AudioCue`] — timeline-triggered audio with an animated volume
//!   envelope.
//! - [`SceneRenderer`] / [`SceneSampler`] — traits the renderer
//!   implements. [`raster_renderer::RasterRenderer`] is the concrete
//!   driver for the vector slice; [`StubRenderer`] is the
//!   always-`Error::Unsupported` placeholder.

pub mod adapt;
pub mod animation;
pub mod audio;
pub mod duration;
pub mod id;
pub mod object;
pub mod ops;
pub mod page;
pub mod paint;
// `raster` was previously gated behind the `raster` cargo feature; with
// the round-2 text-pipeline migration `oxideav-raster` is a hard
// dependency (text rendering goes through it), so the gate is dropped
// here too. The `raster` cargo feature is preserved as a no-op for
// back-compat.
pub mod raster;
pub mod raster_renderer;
pub mod render;
pub mod scene;
pub mod source;
pub mod text;

pub use adapt::{adapt_frame_to, adapt_frame_to_canvas, AdaptedSource};
pub use animation::{AnimatedProperty, Animation, Easing, Keyframe, KeyframeValue, Repeat};
pub use audio::{AudioCue, AudioSource, DuckBus};
pub use duration::{Lifetime, SceneDuration, TimeStamp};
pub use id::ObjectId;
pub use object::{
    BlendMode, Canvas, ClipRect, Effect, ImageSource, LengthUnit, LiveStreamHandle, ObjectKind,
    Sample, SceneObject, Shape, TextRun, Transform, VideoSource,
};
pub use ops::{ExportOp, Operation};
pub use page::Page;
pub use paint::{Gradient, Paint, Stop};
pub use raster::rasterize_vector;
pub use raster_renderer::RasterRenderer;
pub use render::{RenderedFrame, SceneRenderer, SceneSampler, StubRenderer};
pub use scene::{Background, Metadata, Scene};
pub use source::{drive, FnSink, NullSink, RenderedSource, SceneSink, SceneSource, SourceFormat};
pub use text::TextRenderer;
