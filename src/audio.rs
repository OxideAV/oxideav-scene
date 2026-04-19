//! Timeline-triggered audio cues.
//!
//! Audio is not positioned — there's one mix bus per [`Scene`].
//! Each [`AudioCue`] has a trigger timestamp at which it starts
//! playing. The renderer accumulates contributions from every live
//! cue into the scene's output audio buffer.

use std::sync::Arc;

use crate::animation::Animation;
use crate::duration::TimeStamp;

/// One scheduled audio playback.
#[derive(Clone, Debug)]
pub struct AudioCue {
    /// Scene-time at which this cue starts playing.
    pub trigger: TimeStamp,
    pub source: AudioSource,
    /// Animated 0.0..=1.0 volume envelope. A cue with no keyframes
    /// plays at unit gain.
    pub volume: Animation,
    /// Other cues / bus tags to duck while this cue is playing.
    pub duck: Vec<DuckBus>,
    /// Optional explicit stop time. `None` = play to the source's
    /// natural end.
    pub end: Option<TimeStamp>,
}

/// Audio content source.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum AudioSource {
    Path(String),
    EncodedBytes(Arc<[u8]>),
    /// Pre-decoded PCM — interleaved S16.
    PcmS16 {
        sample_rate: u32,
        channels: u8,
        samples: Arc<[i16]>,
    },
    /// Pre-decoded PCM — interleaved F32.
    PcmF32 {
        sample_rate: u32,
        channels: u8,
        samples: Arc<[f32]>,
    },
    /// Generator — sine / noise / silence. Useful for placeholder
    /// beds and quick tests.
    Generator(Generator),
}

/// Simple procedural audio generators.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub enum Generator {
    Silence,
    SineWave { frequency_hz: f32, amplitude: f32 },
    WhiteNoise { amplitude: f32 },
}

/// Ducking reference — attenuate all cues sharing this `bus` tag
/// while the owning cue plays. `reduction_db` is the target level;
/// `attack_ms` / `release_ms` control the envelope around the
/// trigger.
#[derive(Clone, Copy, Debug)]
pub struct DuckBus {
    pub bus: u32,
    pub reduction_db: f32,
    pub attack_ms: u32,
    pub release_ms: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{AnimatedProperty, Easing, Repeat};

    #[test]
    fn default_volume_animation_holds_unity() {
        let cue = AudioCue {
            trigger: 0,
            source: AudioSource::Generator(Generator::Silence),
            volume: Animation::new(
                AnimatedProperty::Volume,
                Vec::new(),
                Easing::Linear,
                Repeat::Once,
            ),
            duck: Vec::new(),
            end: None,
        };
        assert!(cue.volume.sample(0).is_none());
    }
}
