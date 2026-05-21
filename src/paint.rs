//! Typed paint patterns: solid colours, multi-stop gradients.
//!
//! [`Paint`] is the unified colour-source type for renderable surfaces
//! that need richer fills than a single 32-bit RGBA value — gradient
//! backgrounds, shape fills, text colour gradients. The simpler
//! `Shape::Rect { fill: u32, .. }` / `Background::Solid(u32)` API stays
//! in place for the common case; `Paint` is the extension point that
//! lets a renderer reach for a gradient or a future pattern type
//! without touching the call site.
//!
//! ## Gradient stops
//!
//! A [`Stop`] is one colour at one `offset` along the gradient line
//! (`0.0` = start, `1.0` = end). Stops are stored in a `Vec<Stop>` on
//! [`Gradient`]; the renderer interpolates between consecutive stops
//! using linear-RGB lerp (matches `KeyframeValue::Color`'s lerp). For
//! a two-colour gradient pass `&[Stop::new(0.0, c0), Stop::new(1.0,
//! c1)]`; for a multi-stop sunset pass as many stops as you like.
//!
//! Stops are kept in offset-sorted order by the [`Gradient`]
//! constructor; lookups are O(N) but N is bounded (gradients with
//! more than ~10 stops are exotic).
//!
//! ## Variants
//!
//! - [`Gradient::Linear`] — a `(angle_deg, stops)` pair. `angle_deg`
//!   is clockwise from 12 o'clock (matches CSS `linear-gradient`).
//! - [`Gradient::Radial`] — a circular gradient centred at a normalised
//!   `(cx, cy)` with `radius` in normalised canvas units. Useful for
//!   spotlights, vignettes, planet-fade-in effects.
//!
//! Normalised here means 0..=1 across the canvas's smaller axis for
//! `radius`, and 0..=1 across each axis for `(cx, cy)`. The renderer
//! converts to canvas units at composite time.

/// One colour stop along a gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    /// Position along the gradient, `0.0..=1.0`. `0.0` is the start
    /// edge of the gradient line, `1.0` is the end edge.
    pub offset: f32,
    /// `0xRRGGBBAA` colour value.
    pub color: u32,
}

impl Stop {
    /// Convenience constructor — clamps `offset` into `0.0..=1.0`.
    pub fn new(offset: f32, color: u32) -> Self {
        Stop {
            offset: offset.clamp(0.0, 1.0),
            color,
        }
    }
}

/// Multi-stop gradient pattern.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Gradient {
    /// Straight-line gradient. `angle_deg` follows CSS conventions:
    /// `0°` paints bottom-to-top, `90°` left-to-right, etc. Stops are
    /// offset-sorted (see [`Gradient::linear`]).
    Linear { angle_deg: f32, stops: Vec<Stop> },
    /// Circular gradient. `(cx, cy)` is the centre in normalised
    /// canvas coordinates (`0.5, 0.5` is dead-centre). `radius` is in
    /// normalised units of the canvas's smaller axis. Stops are
    /// offset-sorted.
    Radial {
        cx: f32,
        cy: f32,
        radius: f32,
        stops: Vec<Stop>,
    },
}

impl Gradient {
    /// Build a linear gradient. Stops are sorted by offset; duplicates
    /// at the same offset keep their original relative order
    /// (`sort_by` is stable).
    pub fn linear(angle_deg: f32, stops: impl IntoIterator<Item = Stop>) -> Self {
        let mut s: Vec<Stop> = stops.into_iter().collect();
        s.sort_by(|a, b| {
            a.offset
                .partial_cmp(&b.offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Gradient::Linear {
            angle_deg,
            stops: s,
        }
    }

    /// Build a radial gradient. Stops are offset-sorted (see
    /// [`Gradient::linear`]).
    pub fn radial(cx: f32, cy: f32, radius: f32, stops: impl IntoIterator<Item = Stop>) -> Self {
        let mut s: Vec<Stop> = stops.into_iter().collect();
        s.sort_by(|a, b| {
            a.offset
                .partial_cmp(&b.offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Gradient::Radial {
            cx,
            cy,
            radius,
            stops: s,
        }
    }

    /// Borrow the gradient's stop list.
    pub fn stops(&self) -> &[Stop] {
        match self {
            Gradient::Linear { stops, .. } => stops,
            Gradient::Radial { stops, .. } => stops,
        }
    }

    /// Sample the gradient at a normalised position `t ∈ [0, 1]`
    /// along the gradient axis (for linear) or along the radius (for
    /// radial). Out-of-range `t` clamps to the nearest endpoint stop.
    /// Returns `None` if the gradient has no stops.
    ///
    /// Interpolation is linear per channel — the same lerp used by
    /// [`crate::animation::KeyframeValue::Color`] keyframes — so the
    /// behaviour is consistent across the data model.
    pub fn sample(&self, t: f32) -> Option<u32> {
        let stops = self.stops();
        if stops.is_empty() {
            return None;
        }
        let t = t.clamp(0.0, 1.0);
        if t <= stops[0].offset {
            return Some(stops[0].color);
        }
        if t >= stops[stops.len() - 1].offset {
            return Some(stops[stops.len() - 1].color);
        }
        // Find segment: last stop with offset <= t.
        let mut idx = 0;
        for (i, s) in stops.iter().enumerate() {
            if s.offset <= t {
                idx = i;
            } else {
                break;
            }
        }
        let a = &stops[idx];
        let b = &stops[idx + 1];
        let span = b.offset - a.offset;
        let f = if span <= 0.0 {
            0.0
        } else {
            (t - a.offset) / span
        };
        Some(lerp_color(a.color, b.color, f))
    }
}

/// Unified colour-source type. The renderer treats `Solid` as the
/// fast path and dispatches `Gradient` through a per-stop rasteriser.
/// Designed so future pattern variants (image tile, dashed stroke
/// pattern, mesh) can land as new variants without breaking callers
/// — hence `#[non_exhaustive]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// Single colour — `0xRRGGBBAA`.
    Solid(u32),
    /// Multi-stop gradient — linear or radial.
    Gradient(Gradient),
}

impl Paint {
    /// Convenience — wrap a `0xRRGGBBAA` colour as a [`Paint::Solid`].
    pub fn solid(rgba: u32) -> Self {
        Paint::Solid(rgba)
    }

    /// Convenience — wrap a [`Gradient`] as a [`Paint::Gradient`].
    pub fn gradient(g: Gradient) -> Self {
        Paint::Gradient(g)
    }

    /// Resolve to a single colour at a gradient axis position `t`.
    /// For [`Paint::Solid`] returns the colour unchanged regardless
    /// of `t`. Useful for fallback rasterisers that don't implement
    /// gradient shading and want to pick a representative colour.
    pub fn sample(&self, t: f32) -> u32 {
        match self {
            Paint::Solid(rgba) => *rgba,
            Paint::Gradient(g) => g.sample(t).unwrap_or(0),
        }
    }
}

/// Per-channel linear lerp on `0xRRGGBBAA` colours. Mirrors the
/// (private) `lerp_color` in [`crate::animation`] so the gradient
/// shader produces bit-identical colours to the animation channel
/// for the same endpoints.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_clamps_offset() {
        assert_eq!(Stop::new(-0.5, 0).offset, 0.0);
        assert_eq!(Stop::new(2.0, 0).offset, 1.0);
        assert_eq!(Stop::new(0.25, 0).offset, 0.25);
    }

    #[test]
    fn linear_constructor_sorts_stops() {
        let g = Gradient::linear(
            45.0,
            vec![
                Stop::new(1.0, 0xFFFFFFFF),
                Stop::new(0.0, 0x000000FF),
                Stop::new(0.5, 0x808080FF),
            ],
        );
        let s = g.stops();
        assert_eq!(s.len(), 3);
        assert!(s[0].offset <= s[1].offset);
        assert!(s[1].offset <= s[2].offset);
    }

    #[test]
    fn sample_clamps_to_endpoints() {
        let g = Gradient::linear(
            0.0,
            vec![Stop::new(0.0, 0xFF000000), Stop::new(1.0, 0x00FF0000)],
        );
        assert_eq!(g.sample(-1.0), Some(0xFF000000));
        assert_eq!(g.sample(2.0), Some(0x00FF0000));
    }

    #[test]
    fn sample_midpoint_lerps_per_channel() {
        // Black → white at 0.5 → mid-grey.
        let g = Gradient::linear(
            0.0,
            vec![Stop::new(0.0, 0x000000FF), Stop::new(1.0, 0xFFFFFFFF)],
        );
        let mid = g.sample(0.5).unwrap();
        let r = (mid >> 24) & 0xff;
        let g_ = (mid >> 16) & 0xff;
        let b = (mid >> 8) & 0xff;
        let a = mid & 0xff;
        assert!((100..=155).contains(&r), "r={r}");
        assert!((100..=155).contains(&g_), "g={g_}");
        assert!((100..=155).contains(&b), "b={b}");
        assert_eq!(a, 0xff);
    }

    #[test]
    fn sample_picks_correct_segment_with_three_stops() {
        let g = Gradient::linear(
            0.0,
            vec![
                Stop::new(0.0, 0xFF0000FF), // red
                Stop::new(0.5, 0x00FF00FF), // green
                Stop::new(1.0, 0x0000FFFF), // blue
            ],
        );
        // At 0.25 we should be halfway from red to green: ~yellow-ish.
        let v = g.sample(0.25).unwrap();
        let r = (v >> 24) & 0xff;
        let g_ = (v >> 16) & 0xff;
        let b = (v >> 8) & 0xff;
        assert!(r > 100 && r < 155, "r={r}");
        assert!(g_ > 100 && g_ < 155, "g={g_}");
        assert_eq!(b, 0);

        // At 0.75 we should be halfway from green to blue.
        let v = g.sample(0.75).unwrap();
        let r = (v >> 24) & 0xff;
        let g_ = (v >> 16) & 0xff;
        let b = (v >> 8) & 0xff;
        assert_eq!(r, 0);
        assert!(g_ > 100 && g_ < 155, "g={g_}");
        assert!(b > 100 && b < 155, "b={b}");
    }

    #[test]
    fn empty_gradient_returns_none() {
        let g = Gradient::linear(0.0, Vec::<Stop>::new());
        assert!(g.sample(0.5).is_none());
    }

    #[test]
    fn radial_constructor_sorts_stops() {
        let g = Gradient::radial(
            0.5,
            0.5,
            0.5,
            vec![Stop::new(1.0, 0x000000FF), Stop::new(0.0, 0xFFFFFFFF)],
        );
        let s = g.stops();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].offset, 0.0);
        assert_eq!(s[1].offset, 1.0);
        if let Gradient::Radial {
            cx,
            cy,
            radius,
            stops,
        } = &g
        {
            assert_eq!(*cx, 0.5);
            assert_eq!(*cy, 0.5);
            assert_eq!(*radius, 0.5);
            assert_eq!(stops.len(), 2);
        } else {
            panic!("expected Radial");
        }
    }

    #[test]
    fn paint_solid_sample_is_constant() {
        let p = Paint::solid(0x123456FF);
        assert_eq!(p.sample(0.0), 0x123456FF);
        assert_eq!(p.sample(0.5), 0x123456FF);
        assert_eq!(p.sample(1.0), 0x123456FF);
    }

    #[test]
    fn paint_gradient_sample_delegates() {
        let p = Paint::gradient(Gradient::linear(
            0.0,
            vec![Stop::new(0.0, 0x000000FF), Stop::new(1.0, 0xFFFFFFFF)],
        ));
        let v = p.sample(0.5);
        let r = (v >> 24) & 0xff;
        assert!((100..=155).contains(&r));
    }

    #[test]
    fn paint_empty_gradient_samples_zero() {
        let p = Paint::gradient(Gradient::linear(0.0, Vec::<Stop>::new()));
        assert_eq!(p.sample(0.5), 0);
    }
}
