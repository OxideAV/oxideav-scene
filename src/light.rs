//! Typed light primitives.
//!
//! A first 3D-adjacent typed surface on the scene crate. The model is
//! "punctual" — each light is an infinitely small point that emits
//! light in a well-defined direction and intensity — and follows the
//! glTF 2.0 `KHR_lights_punctual` ratified extension, which we treat
//! as the canonical clean-room contract for the three primitive
//! variants. The shape is:
//!
//! - [`Light::Directional`] — emits along the local `-z` axis,
//!   acts as if infinitely far away, no positional attenuation.
//!   Intensity is illuminance (`lm/m²`, lux).
//! - [`Light::Point`] — emits omnidirectionally from a point in
//!   space. Intensity attenuates with distance per the inverse-square
//!   law. Intensity is luminous intensity (`lm/sr`, candela).
//! - [`Light::Spot`] — emits inside a cone along the local `-z` axis,
//!   parameterised by `inner_cone_angle` (falloff begins) and
//!   `outer_cone_angle` (falloff ends). Also attenuates with distance.
//!
//! Surface-only at this round. The renderer doesn't consume lights
//! yet — the type is exposed so 3D-scene readers (glTF importers etc.)
//! have a typed landing place. The renderer-side integration is a
//! follow-up.
//!
//! The [`LightCommon`] block carries the per-instance properties
//! every variant shares (`name`, `color`, `intensity`, `range`) so
//! constructors stay terse and the per-variant payload only carries
//! variant-specific fields.
//!
//! # Example
//!
//! ```
//! use oxideav_scene::light::{Light, LightCommon, SpotParams};
//!
//! let key = Light::Directional {
//!     common: LightCommon::default(),
//! };
//! assert!(key.is_directional());
//!
//! let fill = Light::Spot {
//!     common: LightCommon::default(),
//!     spot: SpotParams::default(),
//! };
//! // Default spot has the spec's documented defaults.
//! if let Light::Spot { spot, .. } = fill {
//!     assert_eq!(spot.inner_cone_angle, 0.0);
//!     assert!((spot.outer_cone_angle - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
//! }
//! ```

/// Properties every punctual light shares.
///
/// All fields are optional in the source data; the defaults track the
/// spec's documented defaults exactly:
///
/// - `name` — empty string.
/// - `color` — `[1.0, 1.0, 1.0]` (linear-space RGB).
/// - `intensity` — `1.0`. Units depend on the variant (see
///   [`Light`]).
/// - `range` — `None`, meaning "no cutoff" / infinite. Only meaningful
///   for [`Light::Point`] and [`Light::Spot`] (directional lights are
///   at infinity and the field is ignored). When `Some(r)`,
///   conforming renderers must clamp the light's contribution to zero
///   for fragments farther than `r` units away.
#[derive(Clone, Debug, PartialEq)]
pub struct LightCommon {
    pub name: String,
    /// Linear-space RGB. Multiplies the spectral intensity.
    pub color: [f32; 3],
    /// Brightness. Units differ per variant — see [`Light`].
    pub intensity: f32,
    /// Distance cutoff. `None` = infinite. Must be `> 0` when set.
    pub range: Option<f32>,
}

impl Default for LightCommon {
    fn default() -> Self {
        LightCommon {
            name: String::new(),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            range: None,
        }
    }
}

/// Variant-specific parameters for a spot light.
///
/// Both angles are measured from the centre of the cone (the local
/// `-z` axis) in radians. Inside `inner_cone_angle` the spot emits
/// at full intensity; between `inner_cone_angle` and
/// `outer_cone_angle` the intensity rolls off smoothly; outside
/// `outer_cone_angle` the contribution is zero.
///
/// Invariants enforced by [`SpotParams::is_valid`]:
///
/// - `inner_cone_angle >= 0.0`
/// - `inner_cone_angle < outer_cone_angle`
/// - `outer_cone_angle <= PI / 2`
///
/// The defaults (`0.0`, `PI/4`) match the spec.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpotParams {
    pub inner_cone_angle: f32,
    pub outer_cone_angle: f32,
}

impl Default for SpotParams {
    fn default() -> Self {
        SpotParams {
            inner_cone_angle: 0.0,
            outer_cone_angle: std::f32::consts::FRAC_PI_4,
        }
    }
}

impl SpotParams {
    /// Build a spot parameter block from raw inner / outer angles.
    /// Convenience — does *not* validate; combine with
    /// [`is_valid`](Self::is_valid) when reading from external data.
    pub const fn new(inner_cone_angle: f32, outer_cone_angle: f32) -> Self {
        SpotParams {
            inner_cone_angle,
            outer_cone_angle,
        }
    }

    /// `true` when the angles satisfy the documented invariants.
    pub fn is_valid(&self) -> bool {
        self.inner_cone_angle >= 0.0
            && self.inner_cone_angle < self.outer_cone_angle
            && self.outer_cone_angle <= std::f32::consts::FRAC_PI_2
    }
}

/// A typed punctual light.
///
/// Three variants, matching the ratified extension's set:
/// [`Directional`](Light::Directional) (parallel rays from infinity),
/// [`Point`](Light::Point) (omnidirectional from a position), and
/// [`Spot`](Light::Spot) (cone-confined directional from a position).
///
/// Position / orientation are not carried here — those come from the
/// owning scene node's world transform. A directional or spot light
/// is implicitly oriented along the local `-z` axis; a point light
/// emits omnidirectionally so orientation is meaningless. Inherited
/// scale from the node affects only position / orientation, never the
/// light's own scalar properties.
///
/// Intensity units:
///
/// | Variant       | Intensity unit         | Physical name        |
/// |---------------|------------------------|----------------------|
/// | `Directional` | `lm/m²`                | Illuminance (lux)    |
/// | `Point`       | `lm/sr`                | Luminous intensity (candela) |
/// | `Spot`        | `lm/sr`                | Luminous intensity (candela) |
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Light {
    /// Parallel-ray light at infinity. `range` on `common` is ignored
    /// (infinite-distance attenuation is the identity).
    Directional { common: LightCommon },
    /// Omnidirectional point light. Inverse-square attenuation;
    /// optional `range` cutoff via `common.range`.
    Point { common: LightCommon },
    /// Cone-confined spot light. Inverse-square attenuation with the
    /// optional `range` cutoff, plus cone-angle falloff between
    /// `spot.inner_cone_angle` and `spot.outer_cone_angle`.
    Spot {
        common: LightCommon,
        spot: SpotParams,
    },
}

impl Light {
    /// Borrow the shared property block.
    pub fn common(&self) -> &LightCommon {
        match self {
            Light::Directional { common }
            | Light::Point { common }
            | Light::Spot { common, .. } => common,
        }
    }

    /// Mutably borrow the shared property block.
    pub fn common_mut(&mut self) -> &mut LightCommon {
        match self {
            Light::Directional { common }
            | Light::Point { common }
            | Light::Spot { common, .. } => common,
        }
    }

    /// `true` for [`Light::Directional`].
    pub fn is_directional(&self) -> bool {
        matches!(self, Light::Directional { .. })
    }

    /// `true` for [`Light::Point`].
    pub fn is_point(&self) -> bool {
        matches!(self, Light::Point { .. })
    }

    /// `true` for [`Light::Spot`].
    pub fn is_spot(&self) -> bool {
        matches!(self, Light::Spot { .. })
    }

    /// `true` when this variant carries a meaningful spatial position
    /// (point + spot — directional lights are at infinity).
    pub fn has_position(&self) -> bool {
        matches!(self, Light::Point { .. } | Light::Spot { .. })
    }

    /// `true` when this variant carries a meaningful local direction
    /// (directional + spot — point lights are omnidirectional).
    pub fn has_direction(&self) -> bool {
        matches!(self, Light::Directional { .. } | Light::Spot { .. })
    }

    /// `true` when this variant honours `common.range` as a distance
    /// cutoff. Directional lights are at infinity and ignore range.
    pub fn honours_range(&self) -> bool {
        matches!(self, Light::Point { .. } | Light::Spot { .. })
    }

    /// Borrow the spot-only parameter block; `None` for the other
    /// variants.
    pub fn spot_params(&self) -> Option<&SpotParams> {
        match self {
            Light::Spot { spot, .. } => Some(spot),
            _ => None,
        }
    }

    /// Distance attenuation factor at `distance` from the light's
    /// position, following the recommended punctual-light formula:
    ///
    /// ```text
    /// attenuation =
    ///     max( min( 1 - (distance / range)^4, 1 ), 0 ) / distance^2
    /// ```
    ///
    /// when `range` is set, falling back to `1 / distance^2` when no
    /// range is configured.
    ///
    /// Returns `1.0` for [`Light::Directional`] (no positional
    /// attenuation) and for `distance <= 0` (the light is at the
    /// fragment; division would blow up — clamp to the
    /// no-attenuation case so callers don't have to special-case it).
    pub fn distance_attenuation(&self, distance: f32) -> f32 {
        if self.is_directional() {
            return 1.0;
        }
        if distance.is_nan() || distance <= 0.0 {
            // covers NaN, 0, negatives — clamp to no-attenuation so
            // callers don't have to special-case degenerate geometry.
            return 1.0;
        }
        let inv_sq = 1.0 / (distance * distance);
        match self.common().range {
            None => inv_sq,
            Some(r) if r > 0.0 => {
                let ratio = distance / r;
                let r4 = ratio * ratio * ratio * ratio;
                let window = (1.0 - r4).clamp(0.0, 1.0);
                window * inv_sq
            }
            // r is zero or negative → spec says range must be >0;
            // treat as no-range rather than producing NaN.
            Some(_) => inv_sq,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec_documented_values() {
        let c = LightCommon::default();
        assert_eq!(c.name, "");
        assert_eq!(c.color, [1.0, 1.0, 1.0]);
        assert!((c.intensity - 1.0).abs() < 1e-6);
        assert!(c.range.is_none());

        let s = SpotParams::default();
        assert_eq!(s.inner_cone_angle, 0.0);
        assert!((s.outer_cone_angle - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
    }

    #[test]
    fn variant_predicates_are_exclusive() {
        let d = Light::Directional {
            common: LightCommon::default(),
        };
        let p = Light::Point {
            common: LightCommon::default(),
        };
        let s = Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        };
        for (l, (is_d, is_p, is_s)) in [
            (&d, (true, false, false)),
            (&p, (false, true, false)),
            (&s, (false, false, true)),
        ] {
            assert_eq!(l.is_directional(), is_d);
            assert_eq!(l.is_point(), is_p);
            assert_eq!(l.is_spot(), is_s);
            // Exactly one predicate must be true.
            assert_eq!(
                (l.is_directional() as u8) + (l.is_point() as u8) + (l.is_spot() as u8),
                1
            );
        }
    }

    #[test]
    fn directional_has_direction_but_no_position_and_ignores_range() {
        let d = Light::Directional {
            common: LightCommon {
                range: Some(50.0),
                ..LightCommon::default()
            },
        };
        assert!(d.has_direction());
        assert!(!d.has_position());
        assert!(!d.honours_range());
        // Distance attenuation is the identity regardless of range
        // because the light is at infinity.
        assert_eq!(d.distance_attenuation(1.0), 1.0);
        assert_eq!(d.distance_attenuation(1_000.0), 1.0);
    }

    #[test]
    fn point_has_position_no_direction_honours_range() {
        let p = Light::Point {
            common: LightCommon::default(),
        };
        assert!(p.has_position());
        assert!(!p.has_direction());
        assert!(p.honours_range());
        assert!(p.spot_params().is_none());
    }

    #[test]
    fn spot_has_position_and_direction_carries_params() {
        let s = Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::new(0.1, 1.0),
        };
        assert!(s.has_position());
        assert!(s.has_direction());
        assert!(s.honours_range());
        let sp = s.spot_params().expect("spot carries params");
        assert!((sp.inner_cone_angle - 0.1).abs() < 1e-6);
        assert!((sp.outer_cone_angle - 1.0).abs() < 1e-6);
    }

    #[test]
    fn spot_params_validity_matches_documented_invariants() {
        // Defaults are valid.
        assert!(SpotParams::default().is_valid());
        // Inner == outer is invalid (strict).
        assert!(!SpotParams::new(0.3, 0.3).is_valid());
        // Inner > outer is invalid.
        assert!(!SpotParams::new(0.6, 0.3).is_valid());
        // Negative inner is invalid.
        assert!(!SpotParams::new(-0.1, 0.5).is_valid());
        // Outer above PI/2 is invalid.
        assert!(!SpotParams::new(0.0, std::f32::consts::FRAC_PI_2 + 0.1).is_valid());
        // Outer exactly PI/2 is the documented upper bound, allowed.
        assert!(SpotParams::new(0.0, std::f32::consts::FRAC_PI_2).is_valid());
    }

    #[test]
    fn distance_attenuation_inverse_square_without_range() {
        let p = Light::Point {
            common: LightCommon::default(),
        };
        // No range → pure 1/d².
        assert!((p.distance_attenuation(1.0) - 1.0).abs() < 1e-6);
        assert!((p.distance_attenuation(2.0) - 0.25).abs() < 1e-6);
        // Degenerate distances clamp to 1.0 (no NaN, no infinity).
        assert_eq!(p.distance_attenuation(0.0), 1.0);
        assert_eq!(p.distance_attenuation(-1.0), 1.0);
        assert_eq!(p.distance_attenuation(f32::NAN), 1.0);
    }

    #[test]
    fn distance_attenuation_with_range_drops_to_zero_at_range() {
        let s = Light::Spot {
            common: LightCommon {
                range: Some(10.0),
                ..LightCommon::default()
            },
            spot: SpotParams::default(),
        };
        // Below range, window factor is < 1 but > 0.
        let a = s.distance_attenuation(5.0);
        assert!(a > 0.0);
        // (1 - (5/10)^4) / 5^2 = (1 - 0.0625) / 25 = 0.0375
        assert!((a - 0.0375).abs() < 1e-4);
        // At range, window is exactly zero.
        assert_eq!(s.distance_attenuation(10.0), 0.0);
        // Past range, window is clamped to zero.
        assert_eq!(s.distance_attenuation(15.0), 0.0);
    }

    #[test]
    fn common_mut_round_trips() {
        let mut l = Light::Point {
            common: LightCommon::default(),
        };
        l.common_mut().intensity = 7.5;
        l.common_mut().name.push_str("key");
        assert!((l.common().intensity - 7.5).abs() < 1e-6);
        assert_eq!(l.common().name, "key");
    }
}
