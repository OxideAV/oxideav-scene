//! Typed PBR material primitives.
//!
//! The companion surface to [`crate::light`]: where lights describe
//! the energy arriving at a surface, a [`Material`] describes how the
//! surface responds. The model is **metallic-roughness** as defined by
//! the glTF 2.0 core specification (§"Materials" + the BRDF appendix),
//! which we treat as the canonical clean-room contract — the same way
//! the light module treats the punctual-lights extension. The shape
//! is:
//!
//! - [`PbrMetallicRoughness`] — `base_color` (linear RGBA factor +
//!   optional sRGB-encoded texture), `metallic` (`0.0` dielectric →
//!   `1.0` metal) and `roughness` (`0.0` smooth → `1.0` rough)
//!   factors, plus the packed metallic-roughness texture (metalness
//!   in B, roughness in G, linear transfer).
//! - [`Material`] — wraps the PBR block with the emissive factor /
//!   texture, the tangent-space normal and occlusion texture slots,
//!   the [`AlphaMode`] coverage policy, and the `double_sided` flag.
//! - [`TextureBinding`] — an *opaque* texture reference: an index
//!   into a caller-managed texture table plus a `TEXCOORD` set
//!   index. The scene crate does not own decoded texture pixels;
//!   importers keep their texture array and resolve the index when
//!   they shade or re-export.
//!
//! Surface-only at this round, mirroring the lights bring-up: no
//! renderer consumes materials yet — the type is exposed so 3D-scene
//! readers / writers have a typed landing place, and the
//! spec-defined *derived* quantities renderers need are exposed as
//! methods ([`PbrMetallicRoughness::diffuse_color`] /
//! [`PbrMetallicRoughness::f0`] /
//! [`PbrMetallicRoughness::alpha_roughness`] /
//! [`PbrMetallicRoughness::fresnel`]) so every consumer derives them
//! identically instead of re-implementing the interpolation rules.
//!
//! Every default tracks the spec's documented defaults exactly, so
//! `Material::default()` is the spec's "material with no properties
//! set": white base color, fully metallic, fully rough, no emission,
//! opaque, single-sided.
//!
//! # Example
//!
//! ```
//! use oxideav_scene::material::{AlphaMode, Material, PbrMetallicRoughness};
//!
//! // A red, fully-rough dielectric.
//! let mat = Material {
//!     name: "red plastic".into(),
//!     pbr: PbrMetallicRoughness {
//!         base_color_factor: [0.8, 0.1, 0.1, 1.0],
//!         metallic_factor: 0.0,
//!         ..PbrMetallicRoughness::default()
//!     },
//!     ..Material::default()
//! };
//! assert!(mat.is_valid());
//! // Dielectric: the diffuse color IS the base color, and the
//! // normal-incidence reflectance is the fixed 4% dielectric value.
//! assert_eq!(mat.pbr.diffuse_color(), [0.8, 0.1, 0.1]);
//! assert_eq!(mat.pbr.f0(), [0.04, 0.04, 0.04]);
//! // Opaque mode ignores alpha entirely.
//! assert_eq!(mat.coverage(0.25), 1.0);
//! ```

/// Reference to a texture in a caller-managed texture table.
///
/// The scene crate does not store decoded texture pixels — a binding
/// is an `index` into whatever texture array the importing /
/// exporting code maintains, plus the `TEXCOORD` set index selecting
/// which UV attribute drives the lookup (default set `0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureBinding {
    /// Index into the caller-managed texture table.
    pub source: usize,
    /// `TEXCOORD` set index used for the coordinate lookup.
    /// Default `0`.
    pub texcoord: usize,
}

impl TextureBinding {
    /// Binding to texture `source` through `TEXCOORD` set 0.
    pub fn new(source: usize) -> Self {
        TextureBinding {
            source,
            texcoord: 0,
        }
    }

    /// Override the `TEXCOORD` set index.
    pub fn with_texcoord(mut self, texcoord: usize) -> Self {
        self.texcoord = texcoord;
        self
    }
}

/// A [`TextureBinding`] for the tangent-space normal map, plus the
/// `scale` applied to the sampled vector's X / Y components
/// (`scaled = normalize((sample * 2 - 1) * (scale, scale, 1))`).
/// Default scale `1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalTextureBinding {
    pub binding: TextureBinding,
    /// X/Y multiplier for the decoded normal vector. Default `1.0`.
    pub scale: f32,
}

impl NormalTextureBinding {
    /// Wrap `binding` with the default scale of `1.0`.
    pub fn new(binding: TextureBinding) -> Self {
        NormalTextureBinding {
            binding,
            scale: 1.0,
        }
    }
}

/// A [`TextureBinding`] for the occlusion map (occlusion sampled from
/// the R channel), plus the `strength` multiplier:
/// `occlusion = 1 + strength * (sample - 1)`. `0.0` disables the
/// map, `1.0` applies it fully. Default `1.0`; valid range
/// `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OcclusionTextureBinding {
    pub binding: TextureBinding,
    /// Occlusion amount in `0.0..=1.0`. Default `1.0`.
    pub strength: f32,
}

impl OcclusionTextureBinding {
    /// Wrap `binding` with the default strength of `1.0`.
    pub fn new(binding: TextureBinding) -> Self {
        OcclusionTextureBinding {
            binding,
            strength: 1.0,
        }
    }

    /// `strength` within `0.0..=1.0` (and finite).
    pub fn is_valid(&self) -> bool {
        self.strength.is_finite() && (0.0..=1.0).contains(&self.strength)
    }
}

/// How a material's alpha value is interpreted at render time.
///
/// The alpha value is the product of the base-color factor's fourth
/// component and the base-color texture's A channel (when present).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AlphaMode {
    /// Alpha is ignored; the surface renders fully opaque. The
    /// default.
    #[default]
    Opaque,
    /// Binary coverage: alpha `>= cutoff` renders fully opaque,
    /// anything below renders fully transparent (nothing is drawn).
    /// A cutoff greater than `1.0` makes the whole material
    /// invisible. The spec default cutoff is
    /// [`AlphaMode::DEFAULT_MASK_CUTOFF`] (`0.5`).
    Mask {
        /// Coverage threshold, `>= 0.0`.
        cutoff: f32,
    },
    /// Alpha composites the surface over the backdrop with the
    /// standard source-over operation.
    Blend,
}

impl AlphaMode {
    /// The spec's default cutoff for [`AlphaMode::Mask`].
    pub const DEFAULT_MASK_CUTOFF: f32 = 0.5;

    /// [`AlphaMode::Mask`] with the default `0.5` cutoff.
    pub fn mask() -> Self {
        AlphaMode::Mask {
            cutoff: Self::DEFAULT_MASK_CUTOFF,
        }
    }

    /// `true` for [`AlphaMode::Opaque`].
    pub fn is_opaque(&self) -> bool {
        matches!(self, AlphaMode::Opaque)
    }

    /// `true` for [`AlphaMode::Mask`].
    pub fn is_mask(&self) -> bool {
        matches!(self, AlphaMode::Mask { .. })
    }

    /// `true` for [`AlphaMode::Blend`].
    pub fn is_blend(&self) -> bool {
        matches!(self, AlphaMode::Blend)
    }

    /// Resolve a raw alpha value into the coverage factor this mode
    /// produces:
    ///
    /// - `Opaque` → `1.0` regardless of `alpha`.
    /// - `Mask { cutoff }` → `1.0` when `alpha >= cutoff`, else
    ///   `0.0` (binary — nothing in between). A NaN alpha fails the
    ///   comparison and yields `0.0`.
    /// - `Blend` → `alpha` clamped to `0.0..=1.0` (non-finite input
    ///   resolves to `0.0`).
    pub fn coverage(&self, alpha: f32) -> f32 {
        match *self {
            AlphaMode::Opaque => 1.0,
            AlphaMode::Mask { cutoff } => {
                if alpha >= cutoff {
                    1.0
                } else {
                    0.0
                }
            }
            AlphaMode::Blend => {
                if alpha.is_finite() {
                    alpha.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            }
        }
    }

    /// `true` when this mode's parameters are in their valid ranges:
    /// the mask cutoff must be finite and `>= 0.0`; the other
    /// variants carry no parameters.
    pub fn is_valid(&self) -> bool {
        match *self {
            AlphaMode::Mask { cutoff } => cutoff.is_finite() && cutoff >= 0.0,
            _ => true,
        }
    }
}

/// The metallic-roughness parameter block.
///
/// Each property is a factor optionally multiplied by a texture
/// lookup; an absent texture samples as `1.0` in every relevant
/// channel, so the factor alone fully describes an untextured
/// material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PbrMetallicRoughness {
    /// Linear-space RGBA multiplier for the base color. Each
    /// component in `0.0..=1.0`. Default `[1.0, 1.0, 1.0, 1.0]`.
    /// The fourth component is the material's alpha coverage,
    /// interpreted per [`Material::alpha_mode`].
    pub base_color_factor: [f32; 4],
    /// Base color texture (RGB sRGB-encoded; A, when present, is
    /// linear alpha coverage). Multiplied component-wise by
    /// `base_color_factor`.
    pub base_color_texture: Option<TextureBinding>,
    /// Metalness in `0.0..=1.0` — `0.0` dielectric, `1.0` metal.
    /// Default `1.0`.
    pub metallic_factor: f32,
    /// Roughness in `0.0..=1.0` — `0.0` perfectly smooth, `1.0`
    /// fully rough. Default `1.0`.
    pub roughness_factor: f32,
    /// Packed metallic-roughness texture: metalness sampled from the
    /// B channel, roughness from the G channel, linear transfer.
    /// Multiplied by the respective factors.
    pub metallic_roughness_texture: Option<TextureBinding>,
}

impl Default for PbrMetallicRoughness {
    fn default() -> Self {
        PbrMetallicRoughness {
            base_color_factor: [1.0, 1.0, 1.0, 1.0],
            base_color_texture: None,
            metallic_factor: 1.0,
            roughness_factor: 1.0,
            metallic_roughness_texture: None,
        }
    }
}

/// Normal-incidence reflectance of a dielectric (the fixed 4% the
/// metallic-roughness model assigns to every non-metal).
pub const DIELECTRIC_F0: f32 = 0.04;

impl PbrMetallicRoughness {
    /// `true` when every factor is finite and within its documented
    /// range (`0.0..=1.0` for each base-color component and for the
    /// metallic / roughness factors).
    pub fn is_valid(&self) -> bool {
        self.base_color_factor
            .iter()
            .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
            && self.metallic_factor.is_finite()
            && (0.0..=1.0).contains(&self.metallic_factor)
            && self.roughness_factor.is_finite()
            && (0.0..=1.0).contains(&self.roughness_factor)
    }

    /// The base color's alpha (coverage) component —
    /// `base_color_factor[3]`.
    pub fn base_alpha(&self) -> f32 {
        self.base_color_factor[3]
    }

    /// The diffuse reflectance `c_diff` of the simplified BRDF:
    /// `lerp(base_color.rgb, black, metallic)` — i.e.
    /// `base_color.rgb * (1 - metallic)`. A pure metal reflects no
    /// light diffusely (all response is specular), a pure dielectric
    /// diffuses its full base color.
    pub fn diffuse_color(&self) -> [f32; 3] {
        let k = 1.0 - self.metallic_factor;
        [
            self.base_color_factor[0] * k,
            self.base_color_factor[1] * k,
            self.base_color_factor[2] * k,
        ]
    }

    /// Normal-incidence specular reflectance `f0`:
    /// `lerp(0.04, base_color.rgb, metallic)`. A pure dielectric
    /// reflects the fixed 4% ([`DIELECTRIC_F0`]) in every channel; a
    /// pure metal reflects its base color.
    pub fn f0(&self) -> [f32; 3] {
        let m = self.metallic_factor;
        [
            DIELECTRIC_F0 + (self.base_color_factor[0] - DIELECTRIC_F0) * m,
            DIELECTRIC_F0 + (self.base_color_factor[1] - DIELECTRIC_F0) * m,
            DIELECTRIC_F0 + (self.base_color_factor[2] - DIELECTRIC_F0) * m,
        ]
    }

    /// The microfacet distribution's `α` parameter — the *squared*
    /// roughness (perceptual roughness maps quadratically onto the
    /// distribution width).
    pub fn alpha_roughness(&self) -> f32 {
        self.roughness_factor * self.roughness_factor
    }

    /// Per-channel Schlick Fresnel term at a given `V·H` cosine:
    /// `F = f0 + (1 - f0) * (1 - |V·H|)^5`. At normal incidence
    /// (`v_dot_h = 1`) this is exactly [`Self::f0`]; at grazing
    /// incidence (`v_dot_h = 0`) every channel approaches `1.0`.
    /// `v_dot_h` is taken by absolute value, so back-facing cosines
    /// behave like their front-facing mirror.
    pub fn fresnel(&self, v_dot_h: f32) -> [f32; 3] {
        let f0 = self.f0();
        let x = (1.0 - v_dot_h.abs()).clamp(0.0, 1.0);
        let x5 = x * x * x * x * x;
        [
            f0[0] + (1.0 - f0[0]) * x5,
            f0[1] + (1.0 - f0[1]) * x5,
            f0[2] + (1.0 - f0[2]) * x5,
        ]
    }
}

/// A complete surface material: the metallic-roughness block plus
/// emission, the normal / occlusion texture slots, alpha coverage
/// policy, and sidedness.
///
/// `Material::default()` is the spec's all-defaults material: white
/// base color, metallic `1.0`, roughness `1.0`, no textures, no
/// emission, [`AlphaMode::Opaque`], single-sided.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Display name. Not required to be unique; empty by default.
    pub name: String,
    /// The metallic-roughness parameter block.
    pub pbr: PbrMetallicRoughness,
    /// Tangent-space normal map. `None` → geometric normals only.
    pub normal_texture: Option<NormalTextureBinding>,
    /// Ambient-occlusion map. `None` → no baked occlusion.
    pub occlusion_texture: Option<OcclusionTextureBinding>,
    /// Emissive texture (RGB sRGB-encoded). Multiplied by
    /// `emissive_factor`.
    pub emissive_texture: Option<TextureBinding>,
    /// Linear-space RGB emission multiplier, each component in
    /// `0.0..=1.0`. Default `[0.0, 0.0, 0.0]` (no emission).
    pub emissive_factor: [f32; 3],
    /// How the base color's alpha is interpreted. Default
    /// [`AlphaMode::Opaque`].
    pub alpha_mode: AlphaMode,
    /// `false` (default) → back faces are culled. `true` → both
    /// sides render, with the back face's normals reversed before
    /// shading.
    pub double_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            name: String::new(),
            pbr: PbrMetallicRoughness::default(),
            normal_texture: None,
            occlusion_texture: None,
            emissive_texture: None,
            emissive_factor: [0.0, 0.0, 0.0],
            alpha_mode: AlphaMode::default(),
            double_sided: false,
        }
    }
}

impl Material {
    /// An all-defaults material carrying `name`.
    pub fn named(name: impl Into<String>) -> Self {
        Material {
            name: name.into(),
            ..Material::default()
        }
    }

    /// `true` when every parameter is finite and within its
    /// documented range: the PBR block validates per
    /// [`PbrMetallicRoughness::is_valid`], each emissive component
    /// must be in `0.0..=1.0`, the alpha mode per
    /// [`AlphaMode::is_valid`], the occlusion strength per
    /// [`OcclusionTextureBinding::is_valid`], and the normal scale
    /// must be finite.
    pub fn is_valid(&self) -> bool {
        self.pbr.is_valid()
            && self
                .emissive_factor
                .iter()
                .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
            && self.alpha_mode.is_valid()
            && self.occlusion_texture.map_or(true, |t| t.is_valid())
            && self.normal_texture.map_or(true, |t| t.scale.is_finite())
    }

    /// `true` when the material emits light: a non-zero emissive
    /// factor, or an emissive texture (whose absent-texture default
    /// of `1.0` would still be silenced by a zero factor — so a
    /// texture only counts together with a non-zero factor).
    pub fn is_emissive(&self) -> bool {
        self.emissive_factor.iter().any(|&c| c > 0.0)
    }

    /// `true` when the material references at least one texture in
    /// any slot — i.e. shading it requires resolving
    /// [`TextureBinding`]s against the caller's texture table.
    pub fn is_textured(&self) -> bool {
        self.pbr.base_color_texture.is_some()
            || self.pbr.metallic_roughness_texture.is_some()
            || self.normal_texture.is_some()
            || self.occlusion_texture.is_some()
            || self.emissive_texture.is_some()
    }

    /// Resolve a raw alpha value through this material's
    /// [`AlphaMode`] — see [`AlphaMode::coverage`].
    pub fn coverage(&self, alpha: f32) -> f32 {
        self.alpha_mode.coverage(alpha)
    }

    /// The coverage produced by the material's own
    /// `base_color_factor` alpha (the untextured case):
    /// `coverage(base_alpha)`.
    pub fn base_coverage(&self) -> f32 {
        self.coverage(self.pbr.base_alpha())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- defaults ---------------------------------------------------

    #[test]
    fn defaults_match_spec() {
        let m = Material::default();
        assert_eq!(m.pbr.base_color_factor, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(m.pbr.metallic_factor, 1.0);
        assert_eq!(m.pbr.roughness_factor, 1.0);
        assert!(m.pbr.base_color_texture.is_none());
        assert!(m.pbr.metallic_roughness_texture.is_none());
        assert_eq!(m.emissive_factor, [0.0, 0.0, 0.0]);
        assert_eq!(m.alpha_mode, AlphaMode::Opaque);
        assert!(!m.double_sided);
        assert!(m.is_valid());
        assert!(!m.is_emissive());
        assert!(!m.is_textured());
    }

    #[test]
    fn texture_binding_defaults() {
        let b = TextureBinding::new(3);
        assert_eq!(b.source, 3);
        assert_eq!(b.texcoord, 0);
        assert_eq!(b.with_texcoord(1).texcoord, 1);
        assert_eq!(NormalTextureBinding::new(b).scale, 1.0);
        assert_eq!(OcclusionTextureBinding::new(b).strength, 1.0);
        assert_eq!(
            AlphaMode::mask(),
            AlphaMode::Mask {
                cutoff: AlphaMode::DEFAULT_MASK_CUTOFF
            }
        );
    }

    // ---- validation -------------------------------------------------

    #[test]
    fn validation_rejects_out_of_range_factors() {
        let mut m = Material {
            pbr: PbrMetallicRoughness {
                metallic_factor: 1.5,
                ..PbrMetallicRoughness::default()
            },
            ..Material::default()
        };
        assert!(!m.is_valid());
        m.pbr.metallic_factor = f32::NAN;
        assert!(!m.is_valid());
        m.pbr.metallic_factor = 0.5;
        assert!(m.is_valid());

        m.pbr.base_color_factor = [0.5, -0.1, 0.5, 1.0];
        assert!(!m.is_valid());
        m.pbr.base_color_factor = [0.5, 0.1, 0.5, 1.0];

        m.emissive_factor = [0.0, 0.0, 1.1];
        assert!(!m.is_valid());
        m.emissive_factor = [0.0, 0.0, 1.0];
        assert!(m.is_valid());

        m.alpha_mode = AlphaMode::Mask { cutoff: -0.5 };
        assert!(!m.is_valid());
        // Cutoff above 1.0 is legal — it renders everything
        // transparent but is not malformed.
        m.alpha_mode = AlphaMode::Mask { cutoff: 1.5 };
        assert!(m.is_valid());
        m.alpha_mode = AlphaMode::Opaque;

        m.occlusion_texture = Some(OcclusionTextureBinding {
            binding: TextureBinding::new(0),
            strength: 2.0,
        });
        assert!(!m.is_valid());
        m.occlusion_texture = None;

        m.normal_texture = Some(NormalTextureBinding {
            binding: TextureBinding::new(0),
            scale: f32::INFINITY,
        });
        assert!(!m.is_valid());
    }

    // ---- alpha coverage ----------------------------------------------

    #[test]
    fn opaque_ignores_alpha() {
        assert_eq!(AlphaMode::Opaque.coverage(0.0), 1.0);
        assert_eq!(AlphaMode::Opaque.coverage(0.4), 1.0);
        assert_eq!(AlphaMode::Opaque.coverage(f32::NAN), 1.0);
    }

    #[test]
    fn mask_is_binary_at_cutoff() {
        let mask = AlphaMode::mask();
        assert_eq!(mask.coverage(0.49), 0.0);
        // The comparison is inclusive: alpha == cutoff passes.
        assert_eq!(mask.coverage(0.5), 1.0);
        assert_eq!(mask.coverage(1.0), 1.0);
        // NaN alpha fails the comparison → transparent.
        assert_eq!(mask.coverage(f32::NAN), 0.0);
        // A cutoff above 1.0 makes everything transparent.
        let strict = AlphaMode::Mask { cutoff: 1.5 };
        assert_eq!(strict.coverage(1.0), 0.0);
    }

    #[test]
    fn blend_passes_alpha_clamped() {
        assert_eq!(AlphaMode::Blend.coverage(0.25), 0.25);
        assert_eq!(AlphaMode::Blend.coverage(-1.0), 0.0);
        assert_eq!(AlphaMode::Blend.coverage(2.0), 1.0);
        assert_eq!(AlphaMode::Blend.coverage(f32::NAN), 0.0);
    }

    #[test]
    fn material_base_coverage_uses_factor_alpha() {
        let m = Material {
            pbr: PbrMetallicRoughness {
                base_color_factor: [1.0, 1.0, 1.0, 0.3],
                ..PbrMetallicRoughness::default()
            },
            alpha_mode: AlphaMode::Blend,
            ..Material::default()
        };
        assert!((m.base_coverage() - 0.3).abs() < 1e-6);
        // Same factor under Opaque: ignored.
        let opaque = Material {
            alpha_mode: AlphaMode::Opaque,
            ..m
        };
        assert_eq!(opaque.base_coverage(), 1.0);
    }

    // ---- derived BRDF inputs ------------------------------------------

    #[test]
    fn dielectric_endpoint() {
        let pbr = PbrMetallicRoughness {
            base_color_factor: [0.8, 0.5, 0.2, 1.0],
            metallic_factor: 0.0,
            ..PbrMetallicRoughness::default()
        };
        assert_eq!(pbr.diffuse_color(), [0.8, 0.5, 0.2]);
        assert_eq!(pbr.f0(), [DIELECTRIC_F0; 3]);
    }

    #[test]
    fn metallic_endpoint() {
        let pbr = PbrMetallicRoughness {
            base_color_factor: [0.9, 0.7, 0.3, 1.0],
            metallic_factor: 1.0,
            ..PbrMetallicRoughness::default()
        };
        assert_eq!(pbr.diffuse_color(), [0.0, 0.0, 0.0]);
        assert_eq!(pbr.f0(), [0.9, 0.7, 0.3]);
    }

    #[test]
    fn intermediate_metalness_interpolates_linearly() {
        let pbr = PbrMetallicRoughness {
            base_color_factor: [1.0, 0.5, 0.04, 1.0],
            metallic_factor: 0.5,
            ..PbrMetallicRoughness::default()
        };
        let cd = pbr.diffuse_color();
        assert!((cd[0] - 0.5).abs() < 1e-6);
        assert!((cd[1] - 0.25).abs() < 1e-6);
        let f0 = pbr.f0();
        assert!((f0[0] - (0.04 + 0.96 * 0.5)).abs() < 1e-6);
        // A channel equal to the dielectric constant is a fixed
        // point of the interpolation.
        assert!((f0[2] - 0.04).abs() < 1e-6);
    }

    #[test]
    fn alpha_roughness_is_squared() {
        let pbr = PbrMetallicRoughness {
            roughness_factor: 0.5,
            ..PbrMetallicRoughness::default()
        };
        assert!((pbr.alpha_roughness() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn fresnel_endpoints_and_monotonicity() {
        let pbr = PbrMetallicRoughness {
            base_color_factor: [0.5, 0.5, 0.5, 1.0],
            metallic_factor: 0.0,
            ..PbrMetallicRoughness::default()
        };
        // Normal incidence reproduces f0 exactly.
        assert_eq!(pbr.fresnel(1.0), pbr.f0());
        // Grazing incidence approaches white.
        let grazing = pbr.fresnel(0.0);
        for c in grazing {
            assert!((c - 1.0).abs() < 1e-6);
        }
        // Monotone non-increasing as the cosine grows.
        let mut prev = f32::INFINITY;
        for i in 0..=10 {
            let f = pbr.fresnel(i as f32 / 10.0)[0];
            assert!(f <= prev + 1e-6);
            prev = f;
        }
        // Negative cosines mirror their positive counterparts.
        assert_eq!(pbr.fresnel(-0.5), pbr.fresnel(0.5));
    }

    #[test]
    fn emissive_and_textured_predicates() {
        let mut m = Material {
            emissive_factor: [0.0, 0.2, 0.0],
            ..Material::default()
        };
        assert!(m.is_emissive());
        // A texture with a zero factor is silenced — not emissive.
        m.emissive_factor = [0.0, 0.0, 0.0];
        m.emissive_texture = Some(TextureBinding::new(0));
        assert!(!m.is_emissive());
        assert!(m.is_textured());

        let mut t = Material::default();
        assert!(!t.is_textured());
        t.pbr.metallic_roughness_texture = Some(TextureBinding::new(1));
        assert!(t.is_textured());
    }
}
