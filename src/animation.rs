//! Keyframe-based property animations.
//!
//! An [`Animation`] targets a single [`AnimatedProperty`] and carries
//! a time-sorted list of [`Keyframe`]s. Between consecutive keyframes
//! the value is interpolated according to the animation's default
//! [`Easing`] — each keyframe can override this for the segment
//! leading up to it.

use crate::duration::TimeStamp;

/// A keyframe track on one property.
#[derive(Clone, Debug)]
pub struct Animation {
    pub property: AnimatedProperty,
    pub keyframes: Vec<Keyframe>,
    pub easing: Easing,
    pub repeat: Repeat,
}

impl Animation {
    /// Build an animation from a property + an unordered keyframe list.
    /// The constructor sorts keyframes by time.
    pub fn new(
        property: AnimatedProperty,
        mut keyframes: Vec<Keyframe>,
        easing: Easing,
        repeat: Repeat,
    ) -> Self {
        keyframes.sort_by_key(|k| k.time);
        Animation {
            property,
            keyframes,
            easing,
            repeat,
        }
    }

    /// Evaluate the animation at scene time `t`. Returns `None` if
    /// the animation has no keyframes. For `Repeat::Once`, times
    /// before the first keyframe clamp to the first value; times
    /// after the last clamp to the last value.
    pub fn sample(&self, t: TimeStamp) -> Option<KeyframeValue> {
        if self.keyframes.is_empty() {
            return None;
        }
        let t = match self.repeat {
            Repeat::Once => t,
            Repeat::Loop => {
                let span = self.span()?;
                if span <= 0 {
                    t
                } else {
                    self.keyframes[0].time + ((t - self.keyframes[0].time).rem_euclid(span))
                }
            }
            Repeat::PingPong => {
                let span = self.span()?;
                if span <= 0 {
                    t
                } else {
                    let offset = (t - self.keyframes[0].time).rem_euclid(span * 2);
                    if offset < span {
                        self.keyframes[0].time + offset
                    } else {
                        self.keyframes[0].time + span * 2 - offset
                    }
                }
            }
        };

        if t <= self.keyframes[0].time {
            return Some(self.keyframes[0].value.clone());
        }
        if t >= self.keyframes[self.keyframes.len() - 1].time {
            return Some(self.keyframes[self.keyframes.len() - 1].value.clone());
        }

        // Find segment: last kf with time <= t.
        let idx = self
            .keyframes
            .binary_search_by_key(&t, |k| k.time)
            .unwrap_or_else(|i| i.saturating_sub(1));
        let (a, b) = (&self.keyframes[idx], &self.keyframes[idx + 1]);
        let span = (b.time - a.time) as f32;
        let raw = if span <= 0.0 {
            0.0
        } else {
            (t - a.time) as f32 / span
        };
        let segment_easing = b.easing.unwrap_or(self.easing);
        let f = segment_easing.apply(raw);
        Some(KeyframeValue::interpolate(&a.value, &b.value, f))
    }

    fn span(&self) -> Option<TimeStamp> {
        let first = self.keyframes.first()?.time;
        let last = self.keyframes.last()?.time;
        Some(last - first)
    }
}

/// Which property a track drives.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnimatedProperty {
    Position,
    Scale,
    Rotation,
    Opacity,
    Skew,
    Anchor,
    Volume,
    /// Scalar parameter of the Nth element in the object's `effects`
    /// chain. `param` is the effect's own parameter name.
    EffectParam {
        effect_idx: usize,
        param: &'static str,
    },
    /// Caller-defined — the [`crate::render::SceneSampler`] decides
    /// how to apply it.
    Custom(String),
}

/// One keyframe — a value at a point in time.
#[derive(Clone, Debug)]
pub struct Keyframe {
    pub time: TimeStamp,
    pub value: KeyframeValue,
    /// Override the animation's default easing for the *incoming*
    /// segment (the one ending at this keyframe).
    pub easing: Option<Easing>,
}

/// Typed keyframe value. `interpolate` is invoked by [`Animation`]
/// between two keyframes of the same variant.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum KeyframeValue {
    Scalar(f32),
    Vec2(f32, f32),
    /// `0xRRGGBBAA` colour, interpolated per-channel in linear space.
    Color(u32),
    /// Held value — interpolate returns `a` below 1.0 and `b` at 1.0.
    /// Use for "step" transitions.
    Discrete(String),
}

impl KeyframeValue {
    pub fn interpolate(a: &KeyframeValue, b: &KeyframeValue, t: f32) -> KeyframeValue {
        match (a, b) {
            (KeyframeValue::Scalar(x), KeyframeValue::Scalar(y)) => {
                KeyframeValue::Scalar(x + (y - x) * t)
            }
            (KeyframeValue::Vec2(x1, y1), KeyframeValue::Vec2(x2, y2)) => {
                KeyframeValue::Vec2(x1 + (x2 - x1) * t, y1 + (y2 - y1) * t)
            }
            (KeyframeValue::Color(a), KeyframeValue::Color(b)) => {
                KeyframeValue::Color(lerp_color(*a, *b, t))
            }
            (KeyframeValue::Discrete(a), KeyframeValue::Discrete(b)) => {
                KeyframeValue::Discrete(if t < 1.0 { a.clone() } else { b.clone() })
            }
            // Different variants → hold the first.
            _ => a.clone(),
        }
    }
}

fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let ac = [
        ((a >> 24) & 0xff) as f32,
        ((a >> 16) & 0xff) as f32,
        ((a >> 8) & 0xff) as f32,
        (a & 0xff) as f32,
    ];
    let bc = [
        ((b >> 24) & 0xff) as f32,
        ((b >> 16) & 0xff) as f32,
        ((b >> 8) & 0xff) as f32,
        (b & 0xff) as f32,
    ];
    let mut out = 0u32;
    for i in 0..4 {
        let v = (ac[i] + (bc[i] - ac[i]) * t).clamp(0.0, 255.0) as u32;
        out |= v << ((3 - i) * 8);
    }
    out
}

/// How an animation loops.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    /// Play once and hold the final value.
    #[default]
    Once,
    /// Wrap back to the start and play forever.
    Loop,
    /// Play forward then backward, forever.
    PingPong,
}

/// Interpolation curve applied to the raw 0..=1 segment fraction.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// CSS `cubic-bezier(x1, y1, x2, y2)`; Adobe After Effects
    /// compatible.
    CubicBezier(f32, f32, f32, f32),
    /// `N` discrete steps (staircase).
    Step(u32),
    /// Hold the starting value until the end, then jump.
    Hold,
}

impl Easing {
    /// Map a raw 0..=1 fraction through the easing curve.
    pub fn apply(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - 2.0 * (1.0 - t) * (1.0 - t)
                }
            }
            Easing::CubicBezier(x1, y1, x2, y2) => cubic_bezier_eval(t, *x1, *y1, *x2, *y2),
            Easing::Step(n) if *n > 0 => {
                let n = *n as f32;
                (t * n).floor() / n
            }
            Easing::Step(_) => 0.0,
            Easing::Hold => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

/// Evaluate a cubic Bezier easing at `t` ∈ [0, 1]. Implements the
/// CSS `cubic-bezier(x1, y1, x2, y2)` spec — converts input `t` into
/// the progress coordinate `x` and returns the `y` at that point.
/// Uses a couple rounds of Newton-Raphson then a binary bracket
/// fallback to stay bit-stable under tight schedules.
fn cubic_bezier_eval(t: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    // B(t) = 3(1-t)²t · P1 + 3(1-t)t² · P2 + t³ · 1
    fn b(t: f32, p1: f32, p2: f32) -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
    }
    // derivative of B(t) with P0=0, P3=1
    fn db(t: f32, p1: f32, p2: f32) -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * p1 + 6.0 * u * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
    }
    // Newton.
    let mut guess = t;
    for _ in 0..4 {
        let d = db(guess, x1, x2);
        if d.abs() < 1e-6 {
            break;
        }
        let x = b(guess, x1, x2) - t;
        guess -= x / d;
        guess = guess.clamp(0.0, 1.0);
    }
    // Bisection fallback — capped at a few iterations to stay cheap.
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..16 {
        let x = b(guess, x1, x2);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            lo = guess;
        } else {
            hi = guess;
        }
        guess = 0.5 * (lo + hi);
    }
    b(guess, y1, y2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_before_first_keyframe_clamps() {
        let anim = Animation::new(
            AnimatedProperty::Opacity,
            vec![
                Keyframe {
                    time: 10,
                    value: KeyframeValue::Scalar(0.0),
                    easing: None,
                },
                Keyframe {
                    time: 20,
                    value: KeyframeValue::Scalar(1.0),
                    easing: None,
                },
            ],
            Easing::Linear,
            Repeat::Once,
        );
        assert_eq!(anim.sample(0), Some(KeyframeValue::Scalar(0.0)));
        assert_eq!(anim.sample(10), Some(KeyframeValue::Scalar(0.0)));
    }

    #[test]
    fn sample_linear_midpoint() {
        let anim = Animation::new(
            AnimatedProperty::Opacity,
            vec![
                Keyframe {
                    time: 0,
                    value: KeyframeValue::Scalar(0.0),
                    easing: None,
                },
                Keyframe {
                    time: 10,
                    value: KeyframeValue::Scalar(10.0),
                    easing: None,
                },
            ],
            Easing::Linear,
            Repeat::Once,
        );
        match anim.sample(5).unwrap() {
            KeyframeValue::Scalar(v) => assert!((v - 5.0).abs() < 1e-3),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn easing_in_out_crosses_half_at_half() {
        assert!((Easing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn cubic_bezier_endpoints() {
        // CSS ease: cubic-bezier(0.25, 0.1, 0.25, 1.0)
        let e = Easing::CubicBezier(0.25, 0.1, 0.25, 1.0);
        assert!((e.apply(0.0) - 0.0).abs() < 1e-3);
        assert!((e.apply(1.0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn repeat_loop_wraps() {
        let anim = Animation::new(
            AnimatedProperty::Opacity,
            vec![
                Keyframe {
                    time: 0,
                    value: KeyframeValue::Scalar(0.0),
                    easing: None,
                },
                Keyframe {
                    time: 10,
                    value: KeyframeValue::Scalar(10.0),
                    easing: None,
                },
            ],
            Easing::Linear,
            Repeat::Loop,
        );
        // At t=15 we've wrapped back to t=5, so value=5.
        match anim.sample(15).unwrap() {
            KeyframeValue::Scalar(v) => assert!((v - 5.0).abs() < 1e-3),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn color_interpolation_halfway() {
        let c = KeyframeValue::interpolate(
            &KeyframeValue::Color(0xFF0000FF),
            &KeyframeValue::Color(0x0000FFFF),
            0.5,
        );
        match c {
            KeyframeValue::Color(v) => {
                let r = (v >> 24) & 0xff;
                let b = (v >> 8) & 0xff;
                assert!(r > 100 && r < 155, "r={r}");
                assert!(b > 100 && b < 155, "b={b}");
            }
            _ => panic!("wrong variant"),
        }
    }
}
