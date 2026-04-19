//! Scene-time primitives.
//!
//! Every timestamp in this crate is an `i64` in the scene's own
//! [`TimeBase`](oxideav_core::TimeBase). Use `scene.time_base` to
//! convert to/from seconds.

/// A timestamp in scene time units (the scene's `time_base` tick
/// granularity).
pub type TimeStamp = i64;

/// How long a scene lasts on its own clock.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneDuration {
    /// Fixed-length scene. Rendered linearly start→end; finishes at
    /// `end` and emits no further frames.
    Finite(TimeStamp),

    /// Streaming / open-ended. The scene runs forever until an
    /// operation explicitly ends it. Used by the RTMP compositor.
    /// Seek / rewind are not supported; content is driven forward
    /// by wall-clock time + control-plane operations.
    Indefinite,
}

impl SceneDuration {
    /// Return `Some(end)` for finite scenes, `None` for streaming.
    pub fn end(&self) -> Option<TimeStamp> {
        match self {
            SceneDuration::Finite(t) => Some(*t),
            SceneDuration::Indefinite => None,
        }
    }

    /// Whether a given timestamp is inside the scene.
    pub fn contains(&self, t: TimeStamp) -> bool {
        t >= 0
            && match self {
                SceneDuration::Finite(end) => t < *end,
                SceneDuration::Indefinite => true,
            }
    }
}

/// Per-object on-screen lifetime. `start <= t < end` — objects
/// outside this half-open interval are not rendered. `end == None`
/// means "until scene end" (works for both finite and indefinite
/// scenes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lifetime {
    pub start: TimeStamp,
    pub end: Option<TimeStamp>,
}

impl Lifetime {
    /// Whether the object should be drawn at time `t`.
    pub fn is_live_at(&self, t: TimeStamp) -> bool {
        t >= self.start && self.end.map_or(true, |end| t < end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_scene_bounds() {
        let d = SceneDuration::Finite(100);
        assert!(d.contains(0));
        assert!(d.contains(99));
        assert!(!d.contains(100));
        assert!(!d.contains(-1));
        assert_eq!(d.end(), Some(100));
    }

    #[test]
    fn indefinite_scene_forever() {
        let d = SceneDuration::Indefinite;
        assert!(d.contains(0));
        assert!(d.contains(i64::MAX));
        assert_eq!(d.end(), None);
    }

    #[test]
    fn lifetime_default_is_always_live_after_zero() {
        let lt = Lifetime::default();
        assert!(lt.is_live_at(0));
        assert!(lt.is_live_at(i64::MAX));
        assert!(!lt.is_live_at(-1));
    }

    #[test]
    fn lifetime_finite_range() {
        let lt = Lifetime {
            start: 10,
            end: Some(20),
        };
        assert!(!lt.is_live_at(9));
        assert!(lt.is_live_at(10));
        assert!(lt.is_live_at(19));
        assert!(!lt.is_live_at(20));
    }
}
