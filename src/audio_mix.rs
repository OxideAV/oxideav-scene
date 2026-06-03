//! Audio cue mixing into a [`crate::render::RenderedFrame`]'s output
//! sample buffer.
//!
//! The [`crate::scene::Scene::audio`] vector carries scheduled
//! [`AudioCue`]s that the renderer mixes down into the
//! [`crate::render::RenderedFrame::audio`] slot — one mono `f32` slice
//! per frame interval, at the scene's declared
//! [`crate::scene::Scene::sample_rate`].
//!
//! # Output shape
//!
//! [`mix_cues`] returns a single-channel `Vec<f32>` covering scene-time
//! `[interval_start, interval_end)`. The renderer calls it once per
//! `render_at(t)`: the first call covers `[0, t)`, every subsequent
//! call covers `[prev_t, t)`. A negative or empty interval (`end <=
//! start`) returns an empty buffer.
//!
//! Channel layout is fixed to mono for now — stereo / multichannel
//! cues are downmixed to a single channel by simple averaging at
//! source-load time. A future round can widen the output to a
//! caller-declared channel count by extending [`crate::source::SourceFormat`].
//!
//! # Supported [`AudioSource`] variants
//!
//! [`AudioSource::PcmS16`], [`AudioSource::PcmF32`], and
//! [`AudioSource::Generator`] (all three sub-variants — `Silence`,
//! `SineWave`, `WhiteNoise`) are mixed. Decoder-bound
//! [`AudioSource::Path`] and [`AudioSource::EncodedBytes`] are skipped
//! silently — pre-decode upstream and feed back via a PCM variant
//! until a decoder-aware audio mixer lands. The skip policy mirrors
//! [`crate::ImageSource::Path`] / [`crate::VideoSource::Path`] on the
//! visual side.
//!
//! # Volume envelope
//!
//! Each [`AudioCue::volume`] is sampled per-output-sample at the
//! sample's scene-time tick (rounded from the sample index via the
//! scene's [`oxideav_core::TimeBase`]). The sampled
//! [`crate::animation::KeyframeValue::Scalar`] is clamped to
//! `0.0..=1.0` and multiplied into the source value; a
//! [`crate::animation::Animation`] with no keyframes is treated as
//! unity gain (no attenuation). Non-scalar variants leave the source
//! untouched.
//!
//! # Sample-rate conversion + clipping
//!
//! PCM cues whose `sample_rate` differs from the scene's are resampled
//! by nearest-neighbour: each output sample picks the source sample at
//! `floor(out_index * src_rate / scene_rate)`. This is the
//! lowest-quality option but is allocation-free and correct enough for
//! the streaming-compositor "sound effect" cue model; a future round
//! can upgrade to linear / sinc as needed. The mixed output is
//! clamped to `[-1.0, 1.0]` after summing all contributing cues so a
//! caller's later WAV encode doesn't overflow.

use oxideav_core::Rational;

use crate::animation::{Animation, KeyframeValue};
use crate::audio::{AudioCue, AudioSource, Generator};
use crate::duration::TimeStamp;
use crate::scene::Scene;

/// Mix every active [`AudioCue`] in `scene` into a mono `f32` buffer
/// covering the scene-time interval `[interval_start, interval_end)`
/// at `scene.sample_rate`.
///
/// Returns an empty buffer when the interval is empty or negative.
pub fn mix_cues(scene: &Scene, interval_start: TimeStamp, interval_end: TimeStamp) -> Vec<f32> {
    if interval_end <= interval_start {
        return Vec::new();
    }
    let sample_rate = scene.sample_rate;
    if sample_rate == 0 {
        return Vec::new();
    }
    let n_samples = sample_count(scene, interval_start, interval_end);
    if n_samples == 0 {
        return Vec::new();
    }
    let mut buf = vec![0.0f32; n_samples];

    for cue in &scene.audio {
        mix_one_cue(scene, cue, interval_start, &mut buf);
    }

    // Clamp on the way out so clipping is consistent for downstream
    // encoders.
    for s in buf.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }

    buf
}

/// Number of output samples covering `[start, end)` at the scene's
/// sample rate, derived through the time base so a 100 ms interval in
/// a `time_base = 1/1000` scene at 48 kHz yields exactly 4800 samples.
///
/// The math is: `(end - start) ticks * (sample_rate Hz) * (time_base
/// seconds per tick)`, executed in i128 to stay overflow-safe for
/// reasonable scene durations. Rounding is toward zero — the per-frame
/// interval length stays a stable function of the input timestamps so
/// successive `mix_cues` calls partition the timeline cleanly.
fn sample_count(scene: &Scene, start: TimeStamp, end: TimeStamp) -> usize {
    let delta = (end as i128) - (start as i128);
    if delta <= 0 {
        return 0;
    }
    let tb: Rational = scene.time_base.0;
    let sample_rate = scene.sample_rate as i128;
    if tb.den == 0 {
        return 0;
    }
    // delta ticks * sample_rate * tb.num / tb.den
    let num = delta
        .saturating_mul(sample_rate)
        .saturating_mul(tb.num as i128);
    let den = tb.den as i128;
    if den == 0 {
        return 0;
    }
    let count = num / den;
    if count <= 0 {
        return 0;
    }
    count as usize
}

/// Tick-time of output sample index `i` inside the rendered interval
/// (i.e. `interval_start + i * (1 / sample_rate) / time_base`).
fn sample_tick(scene: &Scene, interval_start: TimeStamp, i: usize) -> TimeStamp {
    let tb = scene.time_base.0;
    let sample_rate = scene.sample_rate as i128;
    if sample_rate == 0 || tb.num == 0 {
        return interval_start;
    }
    // i / sample_rate seconds → i * tb.den / (sample_rate * tb.num) ticks.
    let num = (i as i128) * (tb.den as i128);
    let den = sample_rate * (tb.num as i128);
    if den == 0 {
        return interval_start;
    }
    let offset = num / den;
    interval_start.saturating_add(offset as TimeStamp)
}

/// Sample the cue's volume envelope at `t`. Returns unity when the
/// envelope has no keyframes (per the contract). Clamps the sampled
/// scalar to `[0.0, 1.0]` so a misconfigured track can't drive the
/// mixer outside the loudspeaker range.
fn volume_at(volume: &Animation, t: TimeStamp) -> f32 {
    match volume.sample(t) {
        Some(KeyframeValue::Scalar(g)) => g.clamp(0.0, 1.0),
        // No keyframes / non-scalar variant → unity gain. This matches
        // the documented default in `AudioCue` ("a cue with no
        // keyframes plays at unit gain").
        _ => 1.0,
    }
}

/// Compute the end of a cue in scene-time ticks. `cue.end` wins if
/// set; otherwise we use the cue's own source duration converted
/// through the scene's time base. Generator cues without an explicit
/// end run forever (return `None`).
fn cue_end(scene: &Scene, cue: &AudioCue) -> Option<TimeStamp> {
    if let Some(e) = cue.end {
        return Some(e);
    }
    natural_end(scene, cue)
}

/// Natural end of a cue from its source duration alone. PCM cues
/// expose `samples.len() / channels`; generator cues run forever
/// (return `None`).
fn natural_end(scene: &Scene, cue: &AudioCue) -> Option<TimeStamp> {
    let (n_frames, src_rate) = match &cue.source {
        AudioSource::PcmS16 {
            samples,
            channels,
            sample_rate,
        } => {
            let chans = (*channels).max(1) as usize;
            (samples.len() / chans, *sample_rate)
        }
        AudioSource::PcmF32 {
            samples,
            channels,
            sample_rate,
        } => {
            let chans = (*channels).max(1) as usize;
            (samples.len() / chans, *sample_rate)
        }
        // Generators don't end on their own; the cue carries the
        // duration via `cue.end`.
        AudioSource::Generator(_) => return None,
        // Decoder-bound: skip (the mixer also skips at sample time).
        _ => return None,
    };
    if src_rate == 0 {
        return None;
    }
    let tb = scene.time_base.0;
    if tb.num == 0 {
        return None;
    }
    // duration_seconds = n_frames / src_rate
    // ticks = duration_seconds / tb_seconds_per_tick
    //       = n_frames * tb.den / (src_rate * tb.num)
    let num = (n_frames as i128) * (tb.den as i128);
    let den = (src_rate as i128) * (tb.num as i128);
    if den == 0 {
        return None;
    }
    Some(cue.trigger.saturating_add((num / den) as TimeStamp))
}

/// Mix one cue into `buf` over the interval starting at
/// `interval_start`. The cue contributes only where its active window
/// `[trigger, end)` overlaps the interval.
fn mix_one_cue(scene: &Scene, cue: &AudioCue, interval_start: TimeStamp, buf: &mut [f32]) {
    let end = cue_end(scene, cue);
    let trigger_sample_index = scene_sample_index_for_tick(scene, cue.trigger);
    let end_sample_index = end.map(|e| scene_sample_index_for_tick(scene, e));
    // Sample index inside the scene timeline of `interval_start`.
    let interval_start_sample_index = scene_sample_index_for_tick(scene, interval_start);
    for (i, slot) in buf.iter_mut().enumerate() {
        // Absolute scene-sample index of this slot.
        let abs_sample = interval_start_sample_index.saturating_add(i as i128);
        if abs_sample < trigger_sample_index {
            continue;
        }
        if let Some(end_idx) = end_sample_index {
            if abs_sample >= end_idx {
                continue;
            }
        }
        // Volume sampled at the slot's tick. Tick granularity is
        // coarser than per-sample for sub-tick sample rates; that's
        // fine — volume envelopes are slow-moving by design.
        let t = sample_tick(scene, interval_start, i);
        let gain = volume_at(&cue.volume, t);
        if gain == 0.0 {
            continue;
        }
        // Per-source offset in scene samples since the cue triggered.
        let elapsed = abs_sample - trigger_sample_index;
        let contribution = match &cue.source {
            AudioSource::Generator(g) => generator_sample_scene_index(g, scene, elapsed),
            AudioSource::PcmS16 {
                samples,
                channels,
                sample_rate,
            } => pcm_s16_sample(samples, *channels, *sample_rate, scene, elapsed),
            AudioSource::PcmF32 {
                samples,
                channels,
                sample_rate,
            } => pcm_f32_sample(samples, *channels, *sample_rate, scene, elapsed),
            // Decoder-bound: skip.
            _ => continue,
        };
        *slot += contribution * gain;
    }
}

/// Scene-sample index corresponding to scene-tick `t`. Maps an
/// integer tick onto the scene-rate sample lattice with
/// floor-toward-zero rounding so per-tick sample boundaries match
/// `sample_tick` going the other direction.
fn scene_sample_index_for_tick(scene: &Scene, t: TimeStamp) -> i128 {
    let tb = scene.time_base.0;
    if tb.den == 0 || scene.sample_rate == 0 {
        return 0;
    }
    // index = t * sample_rate * tb.num / tb.den
    let num = (t as i128)
        .saturating_mul(scene.sample_rate as i128)
        .saturating_mul(tb.num as i128);
    let den = tb.den as i128;
    if den == 0 {
        return 0;
    }
    num / den
}

/// Map a scene-sample offset (since cue trigger) to a source-frame
/// index in a PCM stream running at `src_rate`. Nearest-neighbour
/// rounding toward zero.
fn pcm_source_index(scene: &Scene, elapsed_scene_samples: i128, src_rate: u32) -> Option<usize> {
    if elapsed_scene_samples < 0 {
        return None;
    }
    let scene_rate = scene.sample_rate;
    if scene_rate == 0 {
        return None;
    }
    // src_index = elapsed_scene_samples * src_rate / scene_rate
    let num = elapsed_scene_samples.saturating_mul(src_rate as i128);
    let idx = num / (scene_rate as i128);
    if idx < 0 {
        return None;
    }
    Some(idx as usize)
}

/// Read one mono sample from a `PcmS16` cue at scene-sample offset
/// `elapsed`. Stereo / multichannel sources downmix by averaging
/// across the channels at that frame.
fn pcm_s16_sample(
    samples: &[i16],
    channels: u8,
    sample_rate: u32,
    scene: &Scene,
    elapsed: i128,
) -> f32 {
    if samples.is_empty() || sample_rate == 0 {
        return 0.0;
    }
    let chans = channels.max(1) as usize;
    let Some(frame) = pcm_source_index(scene, elapsed, sample_rate) else {
        return 0.0;
    };
    let base = frame * chans;
    if base + chans > samples.len() {
        return 0.0;
    }
    let sum: i32 = samples[base..base + chans].iter().map(|&s| s as i32).sum();
    let avg = sum as f32 / chans as f32;
    avg / 32768.0
}

/// Read one mono sample from a `PcmF32` cue. Same shape as
/// [`pcm_s16_sample`] but no integer scaling.
fn pcm_f32_sample(
    samples: &[f32],
    channels: u8,
    sample_rate: u32,
    scene: &Scene,
    elapsed: i128,
) -> f32 {
    if samples.is_empty() || sample_rate == 0 {
        return 0.0;
    }
    let chans = channels.max(1) as usize;
    let Some(frame) = pcm_source_index(scene, elapsed, sample_rate) else {
        return 0.0;
    };
    let base = frame * chans;
    if base + chans > samples.len() {
        return 0.0;
    }
    let sum: f32 = samples[base..base + chans].iter().sum();
    sum / chans as f32
}

/// Evaluate one [`Generator`] sample at scene-sample offset `elapsed`
/// since the cue triggered. Driving the phase off an integer
/// scene-sample index keeps it continuous across `mix_cues` chunk
/// boundaries and independent of the scene's tick granularity.
fn generator_sample_scene_index(g: &Generator, scene: &Scene, elapsed: i128) -> f32 {
    if elapsed < 0 {
        return 0.0;
    }
    match g {
        Generator::Silence => 0.0,
        Generator::SineWave {
            frequency_hz,
            amplitude,
        } => {
            let sample_rate = scene.sample_rate;
            if sample_rate == 0 {
                return 0.0;
            }
            let seconds = (elapsed as f64) / (sample_rate as f64);
            let phase = 2.0 * std::f64::consts::PI * (*frequency_hz as f64) * seconds;
            (phase.sin() as f32) * *amplitude
        }
        Generator::WhiteNoise { amplitude } => {
            // Deterministic per scene-sample index — xorshift64* seeded
            // from the index so the same scene rendered at the same
            // sample rate emits the same noise (fuzz-friendly and
            // diff-stable across chunkings).
            let seed = (elapsed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut x = seed.wrapping_add(0xDEAD_BEEF_C0FE_BABE);
            // xorshift64*
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            let n = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
            // Map u64 → [-1, 1).
            let normalised = ((n as i64) as f64) / (i64::MAX as f64);
            (normalised as f32) * *amplitude
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{AnimatedProperty, Easing, Keyframe, Repeat};
    use crate::audio::{AudioCue, AudioSource, Generator};
    use std::sync::Arc;

    fn default_scene(sample_rate: u32) -> Scene {
        Scene {
            sample_rate,
            ..Scene::default()
        }
    }

    fn empty_volume() -> Animation {
        Animation::new(
            AnimatedProperty::Volume,
            Vec::new(),
            Easing::Linear,
            Repeat::Once,
        )
    }

    #[test]
    fn empty_interval_emits_no_samples() {
        let scene = default_scene(48_000);
        assert!(mix_cues(&scene, 100, 100).is_empty());
        assert!(mix_cues(&scene, 200, 100).is_empty());
    }

    #[test]
    fn no_cues_yields_silence_of_expected_length() {
        // 100 ms at 48 kHz time_base 1/1000 → 4800 samples.
        let scene = default_scene(48_000);
        let out = mix_cues(&scene, 0, 100);
        assert_eq!(out.len(), 4800);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn sample_count_at_other_rates() {
        // 1 second at 44.1 kHz time_base 1/1000 → 44100 samples.
        let scene = default_scene(44_100);
        let out = mix_cues(&scene, 0, 1000);
        assert_eq!(out.len(), 44_100);
    }

    #[test]
    fn cue_before_trigger_stays_silent() {
        let mut scene = default_scene(48_000);
        scene.audio.push(AudioCue {
            trigger: 500, // far past the rendered interval
            source: AudioSource::Generator(Generator::SineWave {
                frequency_hz: 440.0,
                amplitude: 0.5,
            }),
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene, 0, 100);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn silence_generator_emits_zero() {
        let mut scene = default_scene(48_000);
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::Generator(Generator::Silence),
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene, 0, 100);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn sine_generator_emits_nonzero() {
        let mut scene = default_scene(48_000);
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::Generator(Generator::SineWave {
                frequency_hz: 1000.0,
                amplitude: 0.8,
            }),
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        // 10 ms covers 10 cycles at 1 kHz → guaranteed non-zero
        // samples through the cycle.
        let out = mix_cues(&scene, 0, 10);
        let lit = out.iter().filter(|&&s| s.abs() > 1e-4).count();
        assert!(lit > 0, "sine generator produced no signal");
        // Amplitude bound — peaks should approach 0.8 but never
        // exceed it on a pure sine.
        let peak = out
            .iter()
            .map(|s| s.abs())
            .fold(0.0_f32, |a: f32, b: f32| a.max(b));
        assert!(
            peak <= 0.8 + 1e-3,
            "sine peak {peak} exceeded amplitude 0.8"
        );
    }

    #[test]
    fn sine_phase_is_continuous_across_chunks() {
        // Render the same 100 ms in one call and in ten 10 ms calls.
        // The latter is the concatenation of per-chunk mixes; samples
        // at the chunk boundaries should equal the same-position
        // sample in the single-call render (modulo float noise).
        let mut scene = default_scene(48_000);
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::Generator(Generator::SineWave {
                frequency_hz: 100.0,
                amplitude: 1.0,
            }),
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let big = mix_cues(&scene, 0, 100);
        let mut chunked = Vec::with_capacity(big.len());
        for k in 0..10 {
            let mut chunk = mix_cues(&scene, k * 10, (k + 1) * 10);
            chunked.append(&mut chunk);
        }
        assert_eq!(chunked.len(), big.len());
        for (a, b) in big.iter().zip(chunked.iter()) {
            assert!((a - b).abs() < 1e-4, "phase discontinuity at boundary");
        }
    }

    #[test]
    fn pcm_f32_cue_at_matching_rate_round_trips() {
        // 1 sample per ms at 1 kHz time base, 1 kHz sample rate → 1 PCM
        // sample per output sample.
        let scene = Scene {
            sample_rate: 1_000,
            ..Scene::default()
        };
        let samples: Arc<[f32]> = Arc::from(vec![0.25; 50].into_boxed_slice());
        let mut scene = scene;
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::PcmF32 {
                sample_rate: 1_000,
                channels: 1,
                samples,
            },
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene, 0, 50);
        assert_eq!(out.len(), 50);
        assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-6));
        // Beyond the source samples → silence (skip past end).
        let beyond = mix_cues(&scene, 50, 100);
        assert!(beyond.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn pcm_s16_cue_scales_to_unit_range() {
        // Full-scale positive S16 (32767) should mix to ≈ 1.0.
        let samples: Arc<[i16]> = Arc::from(vec![32767i16; 10].into_boxed_slice());
        let mut scene = Scene {
            sample_rate: 1_000,
            ..Scene::default()
        };
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::PcmS16 {
                sample_rate: 1_000,
                channels: 1,
                samples,
            },
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene, 0, 10);
        // 32767 / 32768 ≈ 0.99997.
        assert!(out.iter().all(|&s| (s - 0.99997).abs() < 1e-3));
    }

    #[test]
    fn volume_animation_attenuates_pcm() {
        // Pcm payload of 1.0 with a 0.5 volume envelope → 0.5 output.
        let samples: Arc<[f32]> = Arc::from(vec![1.0; 10].into_boxed_slice());
        let volume = Animation::new(
            AnimatedProperty::Volume,
            vec![Keyframe {
                time: 0,
                value: KeyframeValue::Scalar(0.5),
                easing: None,
            }],
            Easing::Linear,
            Repeat::Once,
        );
        let mut scene = Scene {
            sample_rate: 1_000,
            ..Scene::default()
        };
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::PcmF32 {
                sample_rate: 1_000,
                channels: 1,
                samples,
            },
            volume,
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene, 0, 10);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn two_cues_sum_at_overlap() {
        // Two PCM cues, each contributing 0.3, overlap fully → 0.6.
        let s1: Arc<[f32]> = Arc::from(vec![0.3; 10].into_boxed_slice());
        let s2: Arc<[f32]> = Arc::from(vec![0.3; 10].into_boxed_slice());
        let mut scene = Scene {
            sample_rate: 1_000,
            ..Scene::default()
        };
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::PcmF32 {
                sample_rate: 1_000,
                channels: 1,
                samples: s1,
            },
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::PcmF32 {
                sample_rate: 1_000,
                channels: 1,
                samples: s2,
            },
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene, 0, 10);
        assert!(out.iter().all(|&s| (s - 0.6).abs() < 1e-6));
    }

    #[test]
    fn clipping_limits_output_to_unit_range() {
        // Five full-scale 1.0 cues sum to 5.0 raw; clipped to 1.0.
        let mut scene = Scene {
            sample_rate: 1_000,
            ..Scene::default()
        };
        for _ in 0..5 {
            let s: Arc<[f32]> = Arc::from(vec![1.0; 10].into_boxed_slice());
            scene.audio.push(AudioCue {
                trigger: 0,
                source: AudioSource::PcmF32 {
                    sample_rate: 1_000,
                    channels: 1,
                    samples: s,
                },
                volume: empty_volume(),
                duck: Vec::new(),
                end: None,
            });
        }
        let out = mix_cues(&scene, 0, 10);
        assert!(out.iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn explicit_cue_end_truncates_signal() {
        // Sine cue with explicit end at 5 ms — last 5 ms of a 10 ms
        // render is silent.
        let mut scene = default_scene(48_000);
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::Generator(Generator::SineWave {
                frequency_hz: 1_000.0,
                amplitude: 0.5,
            }),
            volume: empty_volume(),
            duck: Vec::new(),
            end: Some(5),
        });
        let out = mix_cues(&scene, 0, 10);
        // 240 samples per ms at 48 kHz. The boundary may fall on
        // either side of a sample so check the back half is silent
        // beyond a small guard band.
        let tail_silence = &out[out.len() / 2 + 24..];
        assert!(tail_silence.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn stereo_pcm_downmixes_by_averaging() {
        // L=1.0, R=-1.0 → 0.0 mono.
        let samples: Arc<[f32]> = Arc::from(vec![1.0, -1.0, 1.0, -1.0].into_boxed_slice());
        let mut scene = Scene {
            sample_rate: 1_000,
            ..Scene::default()
        };
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::PcmF32 {
                sample_rate: 1_000,
                channels: 2,
                samples,
            },
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene, 0, 2);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|&s| s.abs() < 1e-6));
    }

    #[test]
    fn encoded_audio_source_skips_silently() {
        let mut scene = default_scene(1_000);
        let bytes: Arc<[u8]> = Arc::from(vec![1u8; 16].into_boxed_slice());
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::EncodedBytes(bytes),
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene, 0, 10);
        assert!(out.iter().all(|&s| s == 0.0));

        let mut scene2 = default_scene(1_000);
        scene2.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::Path("/tmp/sound.wav".into()),
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        let out = mix_cues(&scene2, 0, 10);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn nearest_neighbour_resamples_to_scene_rate() {
        // 1 kHz source, 2 kHz scene rate → each PCM sample fills two
        // scene samples (nearest neighbour).
        let samples: Arc<[f32]> = Arc::from(vec![0.1, 0.2, 0.3, 0.4].into_boxed_slice());
        let mut scene = Scene {
            sample_rate: 2_000,
            ..Scene::default()
        };
        scene.audio.push(AudioCue {
            trigger: 0,
            source: AudioSource::PcmF32 {
                sample_rate: 1_000,
                channels: 1,
                samples,
            },
            volume: empty_volume(),
            duck: Vec::new(),
            end: None,
        });
        // 4 ms interval → 8 scene samples at 2 kHz.
        let out = mix_cues(&scene, 0, 4);
        assert_eq!(out.len(), 8);
        // Pairs: [0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4]
        let expected = [0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4];
        for (a, b) in out.iter().zip(expected.iter()) {
            assert!((a - b).abs() < 1e-6, "expected {b}, got {a}");
        }
    }
}
