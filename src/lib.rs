//! A time-based composition model for oxideav.
//!
//! This crate is a **scaffold**. The type surface is in place; the
//! renderer is a stub. See [`README.md`](../README.md) for the full
//! design + the three target use cases (PDF pages, RTMP streaming
//! compositor, NLE timeline).
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
//!   implements. Current default ([`StubRenderer`]) always returns
//!   `Error::Unsupported`.

pub mod adapt;
pub mod animation;
pub mod audio;
pub mod duration;
pub mod id;
pub mod object;
pub mod ops;
pub mod render;
pub mod scene;
pub mod source;

pub use adapt::{adapt_frame_to, adapt_frame_to_canvas, AdaptedSource};
pub use animation::{AnimatedProperty, Animation, Easing, Keyframe, KeyframeValue, Repeat};
pub use audio::{AudioCue, AudioSource, DuckBus};
pub use duration::{Lifetime, SceneDuration, TimeStamp};
pub use id::ObjectId;
pub use object::{
    BlendMode, Canvas, ClipRect, Effect, ImageSource, LengthUnit, LiveStreamHandle, ObjectKind,
    SceneObject, Shape, TextRun, Transform, VideoSource,
};
pub use ops::{ExportOp, Operation};
pub use render::{RenderedFrame, SceneRenderer, SceneSampler, StubRenderer};
pub use scene::{Background, Metadata, Scene};
pub use source::{drive, FnSink, NullSink, RenderedSource, SceneSink, SceneSource, SourceFormat};
