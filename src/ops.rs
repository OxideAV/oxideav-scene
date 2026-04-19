//! External-operation DSL for the streaming compositor.
//!
//! The RTMP compositor use case has a control plane that pushes
//! incremental edits into a running scene: add a lower-third, move a
//! logo, fade a sound effect. `Operation` is the in-process
//! representation of that DSL.
//!
//! `ExportOp` is a different beast — it's the output of a
//! [`crate::render::SceneRenderer`] at a given timestamp, describing
//! what should be emitted to the output (a video frame, a PDF text
//! run, a file-format-specific command, …).

use crate::animation::{AnimatedProperty, Animation};
use crate::duration::TimeStamp;
use crate::id::ObjectId;
use crate::object::{SceneObject, Transform};

/// One mutation applied to a running scene.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Operation {
    /// Add a new object. Caller supplies the id (stable, opaque).
    AddObject(Box<SceneObject>),

    /// Remove an object at a specific time. If the scene clock has
    /// already advanced past `at`, the removal is immediate.
    RemoveObject { id: ObjectId, at: TimeStamp },

    /// Replace the base transform wholesale — mostly useful for
    /// snap-to placements that don't need an easing curve.
    SetTransform { id: ObjectId, transform: Transform },

    /// Queue an animation to run against an object's property. The
    /// keyframes land in scene-time; if the scene is `Indefinite`
    /// the caller typically builds them relative to the current
    /// clock.
    Animate { id: ObjectId, animation: Animation },

    /// Cancel a scheduled animation on a property. Lookup happens by
    /// `AnimatedProperty` match — if there's more than one match
    /// (possible with custom names), the first is removed.
    CancelAnimation {
        id: ObjectId,
        property: AnimatedProperty,
    },

    /// Fire an audio cue. Same shape as [`crate::AudioCue`], lifted
    /// here so wire formats can send it inline.
    FireAudio(Box<crate::audio::AudioCue>),

    /// Close a streaming scene. The compositor flushes pending
    /// animations + audio and ends the output stream.
    EndScene,
}

/// Output action at render time.
///
/// A [`crate::render::SceneSampler`] emits one or more of these per
/// sampled timestamp. Raster exports typically see a single
/// `EmitFrame`; vector exports (PDF) see a sequence of structured
/// drawing operations so the structural content survives.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ExportOp {
    /// Send a decoded video frame to the output.
    EmitFrame(oxideav_core::VideoFrame),

    /// Send a block of audio samples. Interleaved F32, scene-bus rate.
    EmitAudio(Vec<f32>),

    /// Raw format-specific output — the exporter gives this meaning.
    /// PDF exporters stuff PDF operators in here (`Tj` glyph shows,
    /// `Do` image draws, `RG`/`f`/`S` colour + fill/stroke).
    Raw {
        format: &'static str,
        payload: Vec<u8>,
    },

    /// Emit a structured text run (font / glyph / position) so the
    /// downstream exporter can write selectable text instead of a
    /// rasterised glyph bitmap.
    EmitText(crate::object::TextRun),
}
