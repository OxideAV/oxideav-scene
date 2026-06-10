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

/// A typed [`Light`] paired with its world-space pose.
///
/// The bare [`Light`] primitive deliberately omits position and
/// orientation — those live on the owning scene node. A
/// [`LightInstance`] bridges that gap for callers that don't carry a
/// full 3D node graph: it carries the light plus the two pieces of
/// world-space pose information the punctual-light contract needs.
///
/// - [`position`](Self::position) — the light's world location, used
///   by [`Light::Point`] and [`Light::Spot`]. Ignored for
///   [`Light::Directional`], which is at infinity.
/// - [`direction`](Self::direction) — the world-space emission
///   direction, used by [`Light::Directional`] and [`Light::Spot`].
///   Ignored for [`Light::Point`], which is omnidirectional. Stored
///   as the actual emission direction (so a node whose `-z` axis
///   has been rotated to `+x` carries `direction = [1, 0, 0]`); the
///   default value `[0.0, 0.0, -1.0]` matches the spec's
///   untransformed local emission axis.
///
/// The instance does NOT participate in the timeline-mode renderer
/// (which is 2D vector / image composition). Renderers that consume
/// 3D scene data — glTF importers and future 3D writers — read this
/// list directly off [`Scene::lights`](crate::Scene::lights).
///
/// # Example
///
/// ```
/// use oxideav_scene::light::{Light, LightCommon, LightInstance};
///
/// let key = LightInstance::new(Light::Directional {
///     common: LightCommon::default(),
/// });
/// // Defaults are the untransformed scene-axis convention: at the
/// // origin, emitting along the -z axis.
/// assert_eq!(key.position, [0.0, 0.0, 0.0]);
/// assert_eq!(key.direction, [0.0, 0.0, -1.0]);
///
/// let lamp = LightInstance::new(Light::Point {
///     common: LightCommon::default(),
/// })
/// .with_position([3.0, 2.5, -1.0]);
/// assert_eq!(lamp.position, [3.0, 2.5, -1.0]);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct LightInstance {
    pub light: Light,
    /// World-space position. Used by [`Light::Point`] and
    /// [`Light::Spot`]; ignored by [`Light::Directional`].
    pub position: [f32; 3],
    /// World-space emission direction. Used by [`Light::Directional`]
    /// and [`Light::Spot`]; ignored by [`Light::Point`]. The default
    /// value `[0.0, 0.0, -1.0]` matches the untransformed local
    /// emission axis. Not required to be unit-length — see
    /// [`normalized_direction`](Self::normalized_direction).
    pub direction: [f32; 3],
}

impl LightInstance {
    /// Construct an instance at the origin emitting along the
    /// untransformed `-z` axis.
    pub const fn new(light: Light) -> Self {
        LightInstance {
            light,
            position: [0.0, 0.0, 0.0],
            direction: [0.0, 0.0, -1.0],
        }
    }

    /// Builder: replace the world-space position. Has no effect on
    /// rendering for [`Light::Directional`].
    pub fn with_position(mut self, position: [f32; 3]) -> Self {
        self.position = position;
        self
    }

    /// Builder: replace the world-space emission direction. Has no
    /// effect on rendering for [`Light::Point`].
    pub fn with_direction(mut self, direction: [f32; 3]) -> Self {
        self.direction = direction;
        self
    }

    /// `true` when [`position`](Self::position) is meaningful for
    /// this instance's [`light`](Self::light) variant.
    pub fn position_is_meaningful(&self) -> bool {
        self.light.has_position()
    }

    /// `true` when [`direction`](Self::direction) is meaningful for
    /// this instance's [`light`](Self::light) variant.
    pub fn direction_is_meaningful(&self) -> bool {
        self.light.has_direction()
    }

    /// Return the emission direction renormalised to unit length.
    /// Returns `None` when the stored vector has length below `1e-12`
    /// (degenerate) or when this instance's variant doesn't honour a
    /// direction — callers can branch on the `None` case rather than
    /// dividing by zero or producing NaNs.
    pub fn normalized_direction(&self) -> Option<[f32; 3]> {
        if !self.direction_is_meaningful() {
            return None;
        }
        let [x, y, z] = self.direction;
        let len_sq = x * x + y * y + z * z;
        if !len_sq.is_finite() || len_sq < 1e-24 {
            return None;
        }
        let inv = 1.0 / len_sq.sqrt();
        Some([x * inv, y * inv, z * inv])
    }

    /// Geometric vector from this instance's [`position`](Self::position)
    /// to `world_point`, returned as `(distance, unit_direction)`.
    ///
    /// The unit direction points *from* the light *towards* the point —
    /// i.e. it is the vector a renderer would dot against the light's
    /// emission axis to find the cosine of incidence. The companion
    /// distance feeds [`Light::distance_attenuation`].
    ///
    /// Returns `None` for:
    ///
    /// - [`Light::Directional`] — the light is at infinity, so there
    ///   is no finite position to take the vector from. Renderers
    ///   should sample the emission direction
    ///   ([`normalized_direction`](Self::normalized_direction)) instead.
    /// - A `world_point` coincident with the light's position (zero
    ///   length, no meaningful direction).
    /// - Any non-finite component (NaN / infinity) in either point.
    pub fn vector_to(&self, world_point: [f32; 3]) -> Option<(f32, [f32; 3])> {
        if !self.light.has_position() {
            return None;
        }
        let dx = world_point[0] - self.position[0];
        let dy = world_point[1] - self.position[1];
        let dz = world_point[2] - self.position[2];
        let len_sq = dx * dx + dy * dy + dz * dz;
        if !len_sq.is_finite() || len_sq < 1e-24 {
            return None;
        }
        let len = len_sq.sqrt();
        let inv = 1.0 / len;
        Some((len, [dx * inv, dy * inv, dz * inv]))
    }

    /// Angular attenuation factor at `world_point` for this light's
    /// emission cone.
    ///
    /// Follows the recommended cosine-interpolation falloff documented
    /// in the punctual-light contract for spot lights:
    ///
    /// ```text
    /// light_angle_scale  = 1 / max(1e-3, cos(inner) - cos(outer))
    /// light_angle_offset = -cos(outer) * light_angle_scale
    /// cd                 = dot(spot_dir, normalize(world_point - position))
    /// angular            = saturate(cd * scale + offset)
    /// angular           *= angular
    /// ```
    ///
    /// where `spot_dir` is the light's normalised emission direction
    /// and `cd` is the cosine of the angle between the emission axis
    /// and the vector from the light to the world point.
    ///
    /// Return contract by variant:
    ///
    /// - [`Light::Spot`] — the formula above, clamped to `[0.0, 1.0]`.
    ///   `1.0` inside `inner_cone_angle`, decreasing to `0.0` at and
    ///   beyond `outer_cone_angle`.
    /// - [`Light::Directional`] / [`Light::Point`] — `1.0`. Directional
    ///   lights have no cone (parallel rays); point lights are
    ///   omnidirectional. Returning `1.0` for these matches the role
    ///   of the cone factor in a `(distance * cone)` product — a
    ///   non-spot light contributes its full distance-attenuated
    ///   energy in every direction it reaches.
    ///
    /// Returns `None` when the world point is coincident with the spot
    /// light's position, or when the stored emission direction is
    /// degenerate — callers should treat that as "geometry too
    /// pathological to shade" and skip the contribution rather than
    /// substitute an arbitrary fallback.
    pub fn cone_attenuation(&self, world_point: [f32; 3]) -> Option<f32> {
        match &self.light {
            Light::Directional { .. } | Light::Point { .. } => Some(1.0),
            Light::Spot { spot, .. } => {
                let spot_dir = self.normalized_direction()?;
                let (_, to_point) = self.vector_to(world_point)?;
                let cd = spot_dir[0] * to_point[0]
                    + spot_dir[1] * to_point[1]
                    + spot_dir[2] * to_point[2];
                let cos_inner = spot.inner_cone_angle.cos();
                let cos_outer = spot.outer_cone_angle.cos();
                // `max(1e-3, …)` matches the documented reference
                // formulation — guards the inner==outer degenerate case
                // from a div-by-zero.
                let denom = (cos_inner - cos_outer).max(1e-3);
                let scale = 1.0 / denom;
                let offset = -cos_outer * scale;
                let angular = (cd * scale + offset).clamp(0.0, 1.0);
                Some(angular * angular)
            }
        }
    }
}

/// The light's per-channel linear-RGB contribution arriving at a world
/// point, folding every attenuation factor the punctual-light contract
/// defines into one result a renderer can multiply against a surface's
/// reflectance.
///
/// The composition is, per channel `c`:
///
/// ```text
/// L_c = color[c] * intensity * distance_attenuation * cone_attenuation
/// ```
///
/// where `distance_attenuation` and `cone_attenuation` come from the
/// already-documented helpers on this type and on [`Light`]:
///
/// - **distance** — `1.0` for [`Light::Directional`] (the light is at
///   infinity and is not attenuated; its illuminance is the same at
///   every point); otherwise the inverse-square law with the optional
///   `range`-cutoff window the contract recommends.
/// - **cone** — `1.0` for [`Light::Directional`] / [`Light::Point`];
///   the cosine-interpolated angular falloff for [`Light::Spot`].
///
/// Return contract:
///
/// - [`Light::Directional`] always returns `Some(color * intensity)` —
///   parallel rays reach every world point with the same energy, so the
///   coordinate is ignored.
/// - [`Light::Point`] / [`Light::Spot`] return `Some([...])` scaled by
///   the distance (and, for spots, cone) attenuation at `world_point`.
///   When a finite `range` is configured and the point lies beyond it,
///   the distance window has already driven the factor to `0.0`, so the
///   result is the zero triple — the contract requires the light to be
///   considered to have no effect past `range`.
/// - `None` when the geometry is too degenerate to shade: a non-finite
///   coordinate, or a `world_point` coincident with a positional
///   light's position (the direction / inverse-square term is
///   undefined). Callers should skip the contribution rather than
///   substitute an arbitrary value. Directional lights never hit this
///   case (they have no position).
///
/// Note the result is *not* clamped to `[0, 1]` per channel: the
/// inverse-square term and the `intensity` are physical and can exceed
/// unity (a bright light close to a surface). Tone-mapping / exposure
/// is the consumer's responsibility.
impl LightInstance {
    pub fn irradiance_at(&self, world_point: [f32; 3]) -> Option<[f32; 3]> {
        let common = self.light.common();
        let base = common.intensity;

        let distance = if self.light.has_position() {
            // A positional light needs a finite, non-coincident vector
            // to the point or the inverse-square / cone terms are
            // undefined; `vector_to` already screens NaN / coincidence.
            let (dist, _) = self.vector_to(world_point)?;
            dist
        } else {
            // Directional: no position, no positional attenuation.
            // Guard only against a non-finite query coordinate so a
            // caller passing garbage gets `None` rather than a NaN
            // triple silently flowing downstream.
            if !world_point.iter().all(|c| c.is_finite()) {
                return None;
            }
            // The distance argument is ignored by `distance_attenuation`
            // for the directional variant (it returns `1.0`), so any
            // finite placeholder works.
            0.0
        };

        let dist_att = self.light.distance_attenuation(distance);
        // `cone_attenuation` returns `None` for the same degenerate
        // geometry `vector_to` rejects; for a positional light we have
        // already passed that gate above, but a spot can still trip the
        // degenerate-direction branch, so propagate the `None`.
        let cone_att = self.cone_attenuation(world_point)?;

        let scale = base * dist_att * cone_att;
        Some([
            common.color[0] * scale,
            common.color[1] * scale,
            common.color[2] * scale,
        ])
    }
}

impl From<Light> for LightInstance {
    fn from(light: Light) -> Self {
        LightInstance::new(light)
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

    #[test]
    fn light_instance_defaults_to_origin_and_minus_z() {
        let inst = LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        });
        assert_eq!(inst.position, [0.0, 0.0, 0.0]);
        assert_eq!(inst.direction, [0.0, 0.0, -1.0]);
    }

    #[test]
    fn light_instance_builders_override_pose() {
        let inst = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        })
        .with_position([1.0, 2.0, 3.0])
        .with_direction([0.0, -1.0, 0.0]);
        assert_eq!(inst.position, [1.0, 2.0, 3.0]);
        assert_eq!(inst.direction, [0.0, -1.0, 0.0]);
    }

    #[test]
    fn light_instance_meaningfulness_tracks_variant() {
        let dir = LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        });
        assert!(!dir.position_is_meaningful());
        assert!(dir.direction_is_meaningful());

        let pt = LightInstance::new(Light::Point {
            common: LightCommon::default(),
        });
        assert!(pt.position_is_meaningful());
        assert!(!pt.direction_is_meaningful());

        let sp = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        });
        assert!(sp.position_is_meaningful());
        assert!(sp.direction_is_meaningful());
    }

    #[test]
    fn light_instance_normalized_direction_unit_length() {
        let inst = LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        })
        .with_direction([3.0, 0.0, -4.0]);
        let n = inst.normalized_direction().expect("non-degenerate");
        // (3, 0, -4) has length 5 → normalised to (0.6, 0.0, -0.8).
        assert!((n[0] - 0.6).abs() < 1e-6);
        assert!(n[1].abs() < 1e-6);
        assert!((n[2] - (-0.8)).abs() < 1e-6);
        // And the result is unit length.
        let mag = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((mag - 1.0).abs() < 1e-6);
    }

    #[test]
    fn light_instance_normalized_direction_none_when_degenerate() {
        let inst = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        })
        .with_direction([0.0, 0.0, 0.0]);
        assert!(inst.normalized_direction().is_none());

        let nan = LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        })
        .with_direction([f32::NAN, 0.0, 0.0]);
        assert!(nan.normalized_direction().is_none());
    }

    #[test]
    fn light_instance_normalized_direction_none_when_variant_ignores_it() {
        // Point lights are omnidirectional → no meaningful direction
        // even when the field carries a non-zero vector.
        let inst = LightInstance::new(Light::Point {
            common: LightCommon::default(),
        })
        .with_direction([1.0, 0.0, 0.0]);
        assert!(inst.normalized_direction().is_none());
    }

    #[test]
    fn vector_to_returns_distance_and_unit_dir_for_point_light() {
        let inst = LightInstance::new(Light::Point {
            common: LightCommon::default(),
        })
        .with_position([1.0, 2.0, 3.0]);
        // Point straight along +x by 5 units → distance 5, unit dir (1, 0, 0).
        let (d, dir) = inst.vector_to([6.0, 2.0, 3.0]).expect("non-degenerate");
        assert!((d - 5.0).abs() < 1e-6);
        assert!((dir[0] - 1.0).abs() < 1e-6);
        assert!(dir[1].abs() < 1e-6);
        assert!(dir[2].abs() < 1e-6);
        // (3, 4, 0) offset → distance 5, unit (0.6, 0.8, 0).
        let (d, dir) = inst.vector_to([4.0, 6.0, 3.0]).expect("non-degenerate");
        assert!((d - 5.0).abs() < 1e-6);
        assert!((dir[0] - 0.6).abs() < 1e-6);
        assert!((dir[1] - 0.8).abs() < 1e-6);
        assert!(dir[2].abs() < 1e-6);
    }

    #[test]
    fn vector_to_none_for_directional_light() {
        // Directional lights are at infinity; vector_to is undefined.
        let inst = LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        });
        assert!(inst.vector_to([1.0, 2.0, 3.0]).is_none());
    }

    #[test]
    fn vector_to_none_on_coincident_or_nonfinite_points() {
        let inst = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        })
        .with_position([1.0, 2.0, 3.0]);
        // Coincident with the light.
        assert!(inst.vector_to([1.0, 2.0, 3.0]).is_none());
        // NaN poisons the squared length.
        assert!(inst.vector_to([f32::NAN, 0.0, 0.0]).is_none());
        // Infinity poisons the squared length.
        assert!(inst.vector_to([f32::INFINITY, 0.0, 0.0]).is_none());
    }

    #[test]
    fn cone_attenuation_unity_for_directional_and_point() {
        let d = LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        });
        assert_eq!(d.cone_attenuation([1.0, 2.0, 3.0]), Some(1.0));

        let p = LightInstance::new(Light::Point {
            common: LightCommon::default(),
        })
        .with_position([0.0, 0.0, 0.0]);
        assert_eq!(p.cone_attenuation([100.0, 50.0, 25.0]), Some(1.0));
    }

    #[test]
    fn cone_attenuation_full_inside_inner_zero_past_outer() {
        // Spot at origin aimed straight down -z (the default direction).
        // Default inner = 0, outer = PI/4, so anything strictly inside
        // the inner cone (i.e. exactly on the axis) returns 1.0, and
        // anything beyond outer returns 0.0.
        let s = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        });
        // On-axis: world point at (0, 0, -10) → direction (0,0,-1) ==
        // spot_dir → cd = 1, well past the inner cosine, saturates to 1.0.
        let on_axis = s.cone_attenuation([0.0, 0.0, -10.0]).expect("on-axis");
        assert!((on_axis - 1.0).abs() < 1e-6);
        // Wide off-axis (90° from -z, in the xz plane): cd = 0 which is
        // well past cos(outer) = cos(PI/4) ≈ 0.707, so saturated to 0.
        let off_axis = s.cone_attenuation([10.0, 0.0, 0.0]).expect("off-axis");
        assert_eq!(off_axis, 0.0);
        // Behind the light: cd = -1, definitely zero.
        let behind = s.cone_attenuation([0.0, 0.0, 10.0]).expect("behind");
        assert_eq!(behind, 0.0);
    }

    #[test]
    fn cone_attenuation_is_monotone_in_the_falloff_band() {
        // Spot at origin, aimed at -z, with a wider cone so we get a
        // meaningful interior falloff region.
        let s = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::new(0.2, 1.0),
        });
        // Sample at angles 0.3, 0.5, 0.8 rad off-axis (xz plane). Each
        // world point at distance 1 from origin: (sin θ, 0, -cos θ).
        let attn = |theta: f32| {
            s.cone_attenuation([theta.sin(), 0.0, -theta.cos()])
                .expect("non-degenerate")
        };
        let a_near = attn(0.3);
        let a_mid = attn(0.5);
        let a_far = attn(0.8);
        // Strictly decreasing across the falloff band.
        assert!(a_near > a_mid, "{a_near} > {a_mid}");
        assert!(a_mid > a_far, "{a_mid} > {a_far}");
        // All in [0, 1].
        for v in [a_near, a_mid, a_far] {
            assert!((0.0..=1.0).contains(&v), "{v} out of [0, 1]");
        }
    }

    #[test]
    fn cone_attenuation_none_on_pathological_geometry() {
        // Coincident point + spot light → vector_to None → cone None.
        let s = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        })
        .with_position([1.0, 1.0, 1.0]);
        assert!(s.cone_attenuation([1.0, 1.0, 1.0]).is_none());

        // Degenerate emission direction → normalized_direction None →
        // cone None.
        let s = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::default(),
        })
        .with_direction([0.0, 0.0, 0.0]);
        assert!(s.cone_attenuation([10.0, 10.0, 10.0]).is_none());
    }

    #[test]
    fn cone_attenuation_handles_inner_equals_outer_degenerate() {
        // Degenerate but constructible cone: inner == outer. The
        // formula's `max(1e-3, …)` guard keeps the result finite; the
        // resulting falloff is a step function at the cone edge.
        let s = LightInstance::new(Light::Spot {
            common: LightCommon::default(),
            spot: SpotParams::new(0.3, 0.3),
        });
        // On-axis: still saturates to 1.0.
        let on_axis = s.cone_attenuation([0.0, 0.0, -1.0]).expect("on-axis");
        assert!((on_axis - 1.0).abs() < 1e-6);
        // Well outside the cone: 0.
        let outside = s.cone_attenuation([1.0, 0.0, 0.0]).expect("outside");
        assert_eq!(outside, 0.0);
    }

    #[test]
    fn from_light_wraps_at_origin() {
        let l = Light::Point {
            common: LightCommon::default(),
        };
        let inst: LightInstance = l.into();
        assert_eq!(inst.position, [0.0, 0.0, 0.0]);
        assert_eq!(inst.direction, [0.0, 0.0, -1.0]);
        assert!(inst.light.is_point());
    }

    #[test]
    fn irradiance_directional_is_color_times_intensity_everywhere() {
        // Directional light: illuminance is constant at every point and
        // the position argument is ignored. color * intensity.
        let inst = LightInstance::new(Light::Directional {
            common: LightCommon {
                color: [0.2, 0.4, 0.8],
                intensity: 3.0,
                ..LightCommon::default()
            },
        });
        let near = inst.irradiance_at([0.0, 0.0, -1.0]).expect("finite");
        let far = inst.irradiance_at([100.0, -50.0, 25.0]).expect("finite");
        // Same energy regardless of where the point is.
        assert_eq!(near, far);
        assert!((near[0] - 0.6).abs() < 1e-6); // 0.2 * 3
        assert!((near[1] - 1.2).abs() < 1e-6); // 0.4 * 3
        assert!((near[2] - 2.4).abs() < 1e-6); // 0.8 * 3
    }

    #[test]
    fn irradiance_directional_none_on_nonfinite_query() {
        let inst = LightInstance::new(Light::Directional {
            common: LightCommon::default(),
        });
        assert!(inst.irradiance_at([f32::NAN, 0.0, 0.0]).is_none());
        assert!(inst.irradiance_at([0.0, f32::INFINITY, 0.0]).is_none());
    }

    #[test]
    fn irradiance_point_follows_inverse_square() {
        // White point light, intensity 4, at the origin, no range.
        let inst = LightInstance::new(Light::Point {
            common: LightCommon {
                intensity: 4.0,
                ..LightCommon::default()
            },
        });
        // At distance 2: 4 * (1/4) = 1.0 per channel (white).
        let at2 = inst.irradiance_at([2.0, 0.0, 0.0]).expect("finite");
        for c in at2 {
            assert!((c - 1.0).abs() < 1e-6, "{c}");
        }
        // At distance 4: 4 * (1/16) = 0.25 per channel — a quarter of
        // the distance-2 value, confirming the inverse-square law.
        let at4 = inst.irradiance_at([0.0, 0.0, 4.0]).expect("finite");
        for c in at4 {
            assert!((c - 0.25).abs() < 1e-6, "{c}");
        }
    }

    #[test]
    fn irradiance_point_color_tints_each_channel() {
        let inst = LightInstance::new(Light::Point {
            common: LightCommon {
                color: [1.0, 0.5, 0.0],
                intensity: 4.0,
                ..LightCommon::default()
            },
        });
        // distance 2 → scale = 4 * 0.25 = 1.0, so result == color.
        let r = inst.irradiance_at([2.0, 0.0, 0.0]).expect("finite");
        assert!((r[0] - 1.0).abs() < 1e-6);
        assert!((r[1] - 0.5).abs() < 1e-6);
        assert!(r[2].abs() < 1e-6);
    }

    #[test]
    fn irradiance_point_zero_at_and_past_range() {
        let inst = LightInstance::new(Light::Point {
            common: LightCommon {
                range: Some(10.0),
                ..LightCommon::default()
            },
        });
        // At range, the distance window is exactly zero → zero triple.
        let at_range = inst.irradiance_at([10.0, 0.0, 0.0]).expect("finite");
        assert_eq!(at_range, [0.0, 0.0, 0.0]);
        // Past range, still zero (window clamps to 0).
        let past = inst.irradiance_at([0.0, 15.0, 0.0]).expect("finite");
        assert_eq!(past, [0.0, 0.0, 0.0]);
        // Inside range, strictly positive.
        let inside = inst.irradiance_at([0.0, 0.0, 5.0]).expect("finite");
        assert!(inside.iter().all(|&c| c > 0.0));
    }

    #[test]
    fn irradiance_point_none_on_coincident_or_nonfinite() {
        let inst = LightInstance::new(Light::Point {
            common: LightCommon::default(),
        })
        .with_position([1.0, 2.0, 3.0]);
        // Coincident with the light → vector_to None → irradiance None.
        assert!(inst.irradiance_at([1.0, 2.0, 3.0]).is_none());
        // Non-finite query coordinate.
        assert!(inst.irradiance_at([f32::NAN, 0.0, 0.0]).is_none());
    }

    #[test]
    fn irradiance_spot_folds_distance_and_cone() {
        // Spot at origin aimed down -z, intensity 4, default cone.
        let inst = LightInstance::new(Light::Spot {
            common: LightCommon {
                intensity: 4.0,
                ..LightCommon::default()
            },
            spot: SpotParams::default(),
        });
        // On-axis at distance 2: cone = 1.0, distance = 1/4, so
        // 4 * 0.25 * 1.0 = 1.0 per channel.
        let on_axis = inst.irradiance_at([0.0, 0.0, -2.0]).expect("finite");
        for c in on_axis {
            assert!((c - 1.0).abs() < 1e-6, "{c}");
        }
        // Just past the outer cone (90° off-axis): cone = 0 → zero
        // triple even though the distance term is non-zero. This is the
        // product of the two attenuations the irradiance helper folds.
        let off = inst.irradiance_at([2.0, 0.0, 0.0]).expect("finite");
        assert_eq!(off, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn irradiance_equals_manual_product_of_factors() {
        // Cross-check the folded result against the individual helpers
        // so the composition stays in lock-step with its parts.
        let inst = LightInstance::new(Light::Spot {
            common: LightCommon {
                color: [0.3, 0.6, 0.9],
                intensity: 2.5,
                range: Some(20.0),
                ..LightCommon::default()
            },
            spot: SpotParams::new(0.2, 1.0),
        })
        .with_position([0.0, 0.0, 0.0])
        .with_direction([0.0, 0.0, -1.0]);

        let p = [0.4_f32, 0.0, -3.0];
        let (dist, _) = inst.vector_to(p).expect("finite");
        let da = inst.light.distance_attenuation(dist);
        let ca = inst.cone_attenuation(p).expect("finite");
        let scale = 2.5 * da * ca;
        let expected = [0.3 * scale, 0.6 * scale, 0.9 * scale];

        let got = inst.irradiance_at(p).expect("finite");
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-6, "{g} vs {e}");
        }
    }
}
