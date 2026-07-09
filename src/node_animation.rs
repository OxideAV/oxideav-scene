//! Keyframe animation of 3D node TRS properties.
//!
//! The animation half of the 3D node surface: [`crate::node`] gives a
//! scene its placement (local transforms composed up a parent chain);
//! this module makes that placement move. The model is the glTF 2.0
//! core animation (§3.11 + Appendix C of the spec), treated as the
//! canonical clean-room contract the same way the light / material /
//! node modules do:
//!
//! - a [`NodeAnimation`] is a set of [`AnimationChannel`]s plus a set
//!   of [`AnimationSampler`]s;
//! - a **sampler** pairs `input` (keyframe timestamps — floating-point
//!   seconds, relative to `t = 0` at the start of the animation) with
//!   `output` (flat keyframe values) and an [`Interpolation`] mode;
//! - a **channel** wires one sampler to one node property: a target
//!   node index into a [`NodeGraph`] and a [`TargetPath`]
//!   (translation / rotation / scale). Within one animation each
//!   `(node, path)` target may be used at most once, and non-animated
//!   properties keep their base values.
//!
//! Sampling follows Appendix C exactly. With `n` keyframes, segment
//! `t_k <= t_c < t_{k+1}`, segment duration `t_d = t_{k+1} - t_k`, and
//! normalized factor `t = (t_c - t_k) / t_d`:
//!
//! - **Step**: `v_t = v_k`.
//! - **Linear**: `v_t = (1 - t) * v_k + t * v_{k+1}` — except
//!   rotations, which use spherical linear interpolation
//!   ([`crate::node::quat_slerp`]) along the short great-circle path.
//! - **CubicSpline**: each keyframe stores an in-tangent `a_k`, a
//!   value `v_k`, and an out-tangent `b_k` (in that order); the
//!   interpolated value is the Hermite form
//!   `v_t = (2t³ - 3t² + 1) v_k + t_d (t³ - 2t² + t) b_k +
//!   (-2t³ + 3t²) v_{k+1} + t_d (t³ - t²) a_{k+1}`, and an
//!   interpolated rotation is normalised before use. A cubic sampler
//!   must have at least 2 keyframes.
//!
//! A timestamp that exists in the input is used as-is (no
//! interpolation), and outside the input range the output clamps to
//! the nearest end — an animation whose earliest keyframe sits at
//! `t = 10` plays its first value from `t = 0`.
//!
//! Only TRS nodes may be animated: a node carrying a pre-baked
//! [`NodeTransform::Matrix`] must not be an animation target (convert
//! with [`NodeTransform::to_trs`] first). The morph-target `weights`
//! path is not modelled yet — [`crate::node::SceneNode`] carries no
//! morph weights; [`TargetPath`] is `#[non_exhaustive]` so the
//! variant can land when the node model grows them.
//!
//! # Example
//!
//! ```
//! use oxideav_scene::node::{NodeGraph, NodeTransform, SceneNode};
//! use oxideav_scene::node_animation::{
//!     AnimationChannel, AnimationSampler, Interpolation, NodeAnimation, TargetPath,
//! };
//!
//! let mut graph = NodeGraph::new();
//! let node = graph.push_root(SceneNode::named("mover"));
//!
//! // Slide +X from 0 to 10 over one second.
//! let anim = NodeAnimation {
//!     name: "slide".into(),
//!     samplers: vec![AnimationSampler {
//!         input: vec![0.0, 1.0],
//!         interpolation: Interpolation::Linear,
//!         output: vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0],
//!     }],
//!     channels: vec![AnimationChannel {
//!         sampler: 0,
//!         target_node: node,
//!         path: TargetPath::Translation,
//!     }],
//! };
//! anim.validate(&graph).unwrap();
//! let gm = anim.global_matrices_at(&graph, 0.5);
//! let origin = gm[node].unwrap().transform_point([0.0, 0.0, 0.0]);
//! assert!((origin[0] - 5.0).abs() < 1e-6);
//! ```

use crate::node::{quat_normalize, quat_slerp, Mat4, NodeGraph, NodeTransform};

/// Which local-transform property of the target node a channel
/// animates. Non-animated properties keep their base values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TargetPath {
    /// The node's translation 3-vector.
    Translation,
    /// The node's rotation unit quaternion (XYZW, `w` scalar).
    Rotation,
    /// The node's scale 3-vector.
    Scale,
}

impl TargetPath {
    /// Number of output floats per keyframe value for this path
    /// (3 for translation / scale, 4 for rotation).
    pub fn components(&self) -> usize {
        match self {
            TargetPath::Translation | TargetPath::Scale => 3,
            TargetPath::Rotation => 4,
        }
    }
}

/// Keyframe interpolation mode (Appendix C of the spec).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interpolation {
    /// Hold the earlier keyframe's value for the whole segment.
    Step,
    /// Componentwise linear interpolation — spherical linear
    /// interpolation for rotations.
    Linear,
    /// Cubic Hermite spline with per-keyframe in/out tangents; the
    /// output stores `a_k, v_k, b_k` triples per keyframe and needs
    /// at least 2 keyframes. Interpolated rotations are normalised.
    CubicSpline,
}

/// One interpolated value, typed by the target path.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum SampledValue {
    /// A translation / scale sample.
    Vec3([f32; 3]),
    /// A rotation sample (unit quaternion, XYZW).
    Quat([f32; 4]),
}

/// Keyframe data: timestamps, interpolation mode, and flat output
/// values.
///
/// `input` holds the keyframe timestamps in strictly increasing
/// order (seconds, `t = 0` is the start of the animation). `output`
/// holds keyframe values flattened component-major: `components()`
/// floats per value, one value per keyframe — except
/// [`Interpolation::CubicSpline`], where each keyframe stores three
/// values (in-tangent, value, out-tangent, in that order).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationSampler {
    /// Keyframe timestamps, strictly increasing, in seconds.
    pub input: Vec<f32>,
    /// Interpolation mode for the segments between keyframes.
    pub interpolation: Interpolation,
    /// Flat keyframe values (see the type docs for the layout).
    pub output: Vec<f32>,
}

impl Default for Interpolation {
    /// The spec default interpolation.
    fn default() -> Self {
        Interpolation::Linear
    }
}

impl AnimationSampler {
    /// Number of stored values per keyframe (3 for cubic-spline
    /// tangent/value/tangent triples, 1 otherwise).
    fn values_per_key(&self) -> usize {
        match self.interpolation {
            Interpolation::CubicSpline => 3,
            _ => 1,
        }
    }

    /// Expected `output` length for `path` given the input length.
    fn expected_output_len(&self, path: TargetPath) -> usize {
        self.input.len() * self.values_per_key() * path.components()
    }

    /// The stored value of keyframe `k` (the `v_k` of the middle slot
    /// for cubic splines), as up to 4 components.
    fn key_value(&self, path: TargetPath, k: usize) -> [f32; 4] {
        let c = path.components();
        let base = match self.interpolation {
            Interpolation::CubicSpline => (k * 3 + 1) * c,
            _ => k * c,
        };
        let mut out = [0.0f32; 4];
        out[..c].copy_from_slice(&self.output[base..base + c]);
        out
    }

    /// The in-tangent `a_k` (cubic splines only).
    fn key_in_tangent(&self, path: TargetPath, k: usize) -> [f32; 4] {
        let c = path.components();
        let base = k * 3 * c;
        let mut out = [0.0f32; 4];
        out[..c].copy_from_slice(&self.output[base..base + c]);
        out
    }

    /// The out-tangent `b_k` (cubic splines only).
    fn key_out_tangent(&self, path: TargetPath, k: usize) -> [f32; 4] {
        let c = path.components();
        let base = (k * 3 + 2) * c;
        let mut out = [0.0f32; 4];
        out[..c].copy_from_slice(&self.output[base..base + c]);
        out
    }

    fn wrap(path: TargetPath, v: [f32; 4]) -> SampledValue {
        match path {
            TargetPath::Rotation => SampledValue::Quat([v[0], v[1], v[2], v[3]]),
            _ => SampledValue::Vec3([v[0], v[1], v[2]]),
        }
    }

    /// Sample this sampler at time `t` for a channel targeting
    /// `path`.
    ///
    /// Implements the Appendix C rules: exact-timestamp values are
    /// used as-is, out-of-range times clamp to the nearest end of the
    /// input range, and segments interpolate per
    /// [`Interpolation`] (slerp for linear rotations; normalisation
    /// after cubic rotation interpolation). Returns `None` when the
    /// sampler is malformed (empty input, output length not matching
    /// `path`, cubic spline with fewer than 2 keyframes) — call
    /// [`NodeAnimation::validate`] to get the precise defect.
    pub fn sample(&self, path: TargetPath, t: f32) -> Option<SampledValue> {
        let n = self.input.len();
        if n == 0 || self.output.len() != self.expected_output_len(path) {
            return None;
        }
        if self.interpolation == Interpolation::CubicSpline && n < 2 {
            return None;
        }

        // Clamp outside the input range to the nearest end.
        if t <= self.input[0] {
            let v = self.key_value(path, 0);
            return Some(Self::wrap(
                path,
                if path == TargetPath::Rotation {
                    quat_normalize(v)
                } else {
                    v
                },
            ));
        }
        if t >= self.input[n - 1] {
            let v = self.key_value(path, n - 1);
            return Some(Self::wrap(
                path,
                if path == TargetPath::Rotation {
                    quat_normalize(v)
                } else {
                    v
                },
            ));
        }

        // Segment lookup: k such that input[k] <= t < input[k + 1].
        // An exact hit uses the keyframe value as-is.
        let k = match self
            .input
            .binary_search_by(|probe| probe.partial_cmp(&t).expect("validated finite input"))
        {
            Ok(exact) => {
                let v = self.key_value(path, exact);
                return Some(Self::wrap(
                    path,
                    if path == TargetPath::Rotation {
                        quat_normalize(v)
                    } else {
                        v
                    },
                ));
            }
            Err(insertion) => insertion - 1,
        };

        let td = self.input[k + 1] - self.input[k];
        let u = (t - self.input[k]) / td;
        let c = path.components();

        let v = match self.interpolation {
            Interpolation::Step => self.key_value(path, k),
            Interpolation::Linear => {
                let vk = self.key_value(path, k);
                let vk1 = self.key_value(path, k + 1);
                if path == TargetPath::Rotation {
                    quat_slerp(vk, vk1, u)
                } else {
                    let mut out = [0.0f32; 4];
                    for (i, slot) in out.iter_mut().enumerate().take(c) {
                        *slot = (1.0 - u) * vk[i] + u * vk1[i];
                    }
                    out
                }
            }
            Interpolation::CubicSpline => {
                let vk = self.key_value(path, k);
                let vk1 = self.key_value(path, k + 1);
                let bk = self.key_out_tangent(path, k);
                let ak1 = self.key_in_tangent(path, k + 1);
                let (u2, u3) = (u * u, u * u * u);
                // Hermite basis weights (Appendix C.5).
                let w_vk = 2.0 * u3 - 3.0 * u2 + 1.0;
                let w_bk = td * (u3 - 2.0 * u2 + u);
                let w_vk1 = -2.0 * u3 + 3.0 * u2;
                let w_ak1 = td * (u3 - u2);
                let mut out = [0.0f32; 4];
                for (i, slot) in out.iter_mut().enumerate().take(c) {
                    *slot = w_vk * vk[i] + w_bk * bk[i] + w_vk1 * vk1[i] + w_ak1 * ak1[i];
                }
                if path == TargetPath::Rotation {
                    out = quat_normalize(out);
                }
                out
            }
        };
        Some(Self::wrap(path, v))
    }
}

/// Wires one [`AnimationSampler`] to one node property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationChannel {
    /// Index into the owning [`NodeAnimation::samplers`].
    pub sampler: usize,
    /// Index of the animated node in the target [`NodeGraph`].
    pub target_node: usize,
    /// Which property of the node this channel drives.
    pub path: TargetPath,
}

/// A named set of channels + samplers animating one [`NodeGraph`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeAnimation {
    /// Optional human-readable name (e.g. `"Walk"`).
    pub name: String,
    /// The keyframe data the channels draw from.
    pub samplers: Vec<AnimationSampler>,
    /// The property bindings.
    pub channels: Vec<AnimationChannel>,
}

/// A defect found by [`NodeAnimation::validate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NodeAnimationError {
    /// A channel references a sampler index past the samplers array.
    SamplerOutOfRange {
        /// The offending channel index.
        channel: usize,
        /// The out-of-range sampler index it references.
        sampler: usize,
    },
    /// A channel targets a node index past the graph's node array.
    NodeOutOfRange {
        /// The offending channel index.
        channel: usize,
        /// The out-of-range node index it targets.
        node: usize,
    },
    /// Two channels animate the same `(node, path)` target.
    DuplicateTarget {
        /// The doubly-animated node.
        node: usize,
        /// The doubly-animated property.
        path: TargetPath,
    },
    /// A channel targets a node whose transform is the pre-baked
    /// matrix form — only TRS properties may be animated.
    MatrixNodeTargeted {
        /// The offending channel index.
        channel: usize,
        /// The matrix-form node it targets.
        node: usize,
    },
    /// A sampler has no keyframes.
    EmptyInput {
        /// The offending sampler index.
        sampler: usize,
    },
    /// A sampler's input timestamps are not strictly increasing.
    NonIncreasingInput {
        /// The offending sampler index.
        sampler: usize,
    },
    /// A sampler's input or output carries a NaN / infinity.
    NonFiniteData {
        /// The offending sampler index.
        sampler: usize,
    },
    /// A sampler's output length does not match
    /// `keyframes x components x (3 if cubic)` for the channel's
    /// path.
    OutputLengthMismatch {
        /// The offending sampler index.
        sampler: usize,
        /// The length the channel's path requires.
        expected: usize,
        /// The actual output length.
        got: usize,
    },
    /// A cubic-spline sampler has fewer than 2 keyframes.
    CubicSplineTooShort {
        /// The offending sampler index.
        sampler: usize,
    },
}

impl std::fmt::Display for NodeAnimationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeAnimationError::SamplerOutOfRange { channel, sampler } => {
                write!(
                    f,
                    "channel {channel} references sampler {sampler} out of range"
                )
            }
            NodeAnimationError::NodeOutOfRange { channel, node } => {
                write!(f, "channel {channel} targets node {node} out of range")
            }
            NodeAnimationError::DuplicateTarget { node, path } => {
                write!(
                    f,
                    "node {node} {path:?} is animated by more than one channel"
                )
            }
            NodeAnimationError::MatrixNodeTargeted { channel, node } => {
                write!(
                    f,
                    "channel {channel} targets node {node}, which carries a matrix transform (only TRS may be animated)"
                )
            }
            NodeAnimationError::EmptyInput { sampler } => {
                write!(f, "sampler {sampler} has no keyframes")
            }
            NodeAnimationError::NonIncreasingInput { sampler } => {
                write!(f, "sampler {sampler} input is not strictly increasing")
            }
            NodeAnimationError::NonFiniteData { sampler } => {
                write!(f, "sampler {sampler} carries NaN / infinite data")
            }
            NodeAnimationError::OutputLengthMismatch {
                sampler,
                expected,
                got,
            } => {
                write!(
                    f,
                    "sampler {sampler} output length {got} does not match expected {expected}"
                )
            }
            NodeAnimationError::CubicSplineTooShort { sampler } => {
                write!(
                    f,
                    "cubic-spline sampler {sampler} has fewer than 2 keyframes"
                )
            }
        }
    }
}

impl std::error::Error for NodeAnimationError {}

impl NodeAnimation {
    /// Check this animation against `graph`, returning the first
    /// defect found.
    ///
    /// Verified: channel sampler / node indices in range, no
    /// duplicate `(node, path)` target, no matrix-form node targeted,
    /// and per referenced sampler: at least one keyframe, strictly
    /// increasing finite input, finite output, output length matching
    /// the channel path's component count (tripled for cubic
    /// splines), and at least 2 keyframes for cubic splines.
    pub fn validate(&self, graph: &NodeGraph) -> Result<(), NodeAnimationError> {
        let mut seen: Vec<(usize, TargetPath)> = Vec::with_capacity(self.channels.len());
        for (ci, ch) in self.channels.iter().enumerate() {
            let Some(sampler) = self.samplers.get(ch.sampler) else {
                return Err(NodeAnimationError::SamplerOutOfRange {
                    channel: ci,
                    sampler: ch.sampler,
                });
            };
            let Some(node) = graph.node(ch.target_node) else {
                return Err(NodeAnimationError::NodeOutOfRange {
                    channel: ci,
                    node: ch.target_node,
                });
            };
            if seen.contains(&(ch.target_node, ch.path)) {
                return Err(NodeAnimationError::DuplicateTarget {
                    node: ch.target_node,
                    path: ch.path,
                });
            }
            seen.push((ch.target_node, ch.path));
            if node.transform.is_matrix() {
                return Err(NodeAnimationError::MatrixNodeTargeted {
                    channel: ci,
                    node: ch.target_node,
                });
            }

            // Sampler checks, in the context of this channel's path.
            let s = ch.sampler;
            if sampler.input.is_empty() {
                return Err(NodeAnimationError::EmptyInput { sampler: s });
            }
            if !sampler.input.iter().all(|v| v.is_finite())
                || !sampler.output.iter().all(|v| v.is_finite())
            {
                return Err(NodeAnimationError::NonFiniteData { sampler: s });
            }
            if sampler.input.windows(2).any(|w| w[0] >= w[1]) {
                return Err(NodeAnimationError::NonIncreasingInput { sampler: s });
            }
            if sampler.interpolation == Interpolation::CubicSpline && sampler.input.len() < 2 {
                return Err(NodeAnimationError::CubicSplineTooShort { sampler: s });
            }
            let expected = sampler.expected_output_len(ch.path);
            if sampler.output.len() != expected {
                return Err(NodeAnimationError::OutputLengthMismatch {
                    sampler: s,
                    expected,
                    got: sampler.output.len(),
                });
            }
        }
        Ok(())
    }

    /// The animation's span: the largest keyframe timestamp across
    /// all samplers referenced by a channel (`0.0` for an empty
    /// animation). Playback conventionally runs `0.0..=duration()`.
    pub fn duration(&self) -> f32 {
        self.channels
            .iter()
            .filter_map(|ch| self.samplers.get(ch.sampler))
            .filter_map(|s| s.input.last().copied())
            .fold(0.0, f32::max)
    }

    /// Every node's local transform at time `t`: the graph's base
    /// transforms with each animated property replaced by its sampled
    /// value. Non-animated properties keep their base values.
    ///
    /// Channels that cannot apply (out-of-range indices, matrix-form
    /// target, malformed sampler) are skipped — run
    /// [`NodeAnimation::validate`] first to surface those as typed
    /// errors.
    pub fn local_transforms_at(&self, graph: &NodeGraph, t: f32) -> Vec<NodeTransform> {
        let mut locals: Vec<NodeTransform> = graph.nodes.iter().map(|n| n.transform).collect();
        for ch in &self.channels {
            let Some(sampler) = self.samplers.get(ch.sampler) else {
                continue;
            };
            let Some(value) = sampler.sample(ch.path, t) else {
                continue;
            };
            let Some(slot) = locals.get_mut(ch.target_node) else {
                continue;
            };
            let NodeTransform::Trs {
                translation,
                rotation,
                scale,
            } = slot
            else {
                continue; // matrix nodes must not be animated
            };
            match (ch.path, value) {
                (TargetPath::Translation, SampledValue::Vec3(v)) => *translation = v,
                (TargetPath::Rotation, SampledValue::Quat(q)) => *rotation = q,
                (TargetPath::Scale, SampledValue::Vec3(v)) => *scale = v,
                _ => {}
            }
        }
        locals
    }

    /// Every node's **global** (world-space) matrix at time `t`:
    /// [`NodeAnimation::local_transforms_at`] composed up the parent
    /// chain exactly like [`NodeGraph::global_matrices`] (`None` for
    /// orphans; cycle-safe).
    pub fn global_matrices_at(&self, graph: &NodeGraph, t: f32) -> Vec<Option<Mat4>> {
        let locals = self.local_transforms_at(graph, t);
        let mut out: Vec<Option<Mat4>> = vec![None; graph.nodes.len()];
        let mut visited = vec![false; graph.nodes.len()];
        for &root in &graph.roots {
            descend_posed(graph, &locals, root, Mat4::IDENTITY, &mut visited, &mut out);
        }
        out
    }
}

/// Depth-first accumulation of `parent_global * local` over the
/// animated local transforms, entering each node at most once.
fn descend_posed(
    graph: &NodeGraph,
    locals: &[NodeTransform],
    current: usize,
    acc_parent: Mat4,
    visited: &mut [bool],
    out: &mut [Option<Mat4>],
) {
    let Some(node) = graph.nodes.get(current) else {
        return;
    };
    if visited[current] {
        return;
    }
    visited[current] = true;
    let global = acc_parent.mul(&locals[current].local_matrix());
    out[current] = Some(global);
    for &child in &node.children {
        descend_posed(graph, locals, child, global, visited, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{quat_from_axis_angle, SceneNode};
    use std::f32::consts::FRAC_PI_2;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn approx3(a: [f32; 3], b: [f32; 3]) -> bool {
        approx(a[0], b[0]) && approx(a[1], b[1]) && approx(a[2], b[2])
    }

    fn vec3(v: SampledValue) -> [f32; 3] {
        match v {
            SampledValue::Vec3(v) => v,
            other => panic!("expected Vec3, got {other:?}"),
        }
    }

    fn quat(v: SampledValue) -> [f32; 4] {
        match v {
            SampledValue::Quat(q) => q,
            other => panic!("expected Quat, got {other:?}"),
        }
    }

    fn linear_vec3(input: Vec<f32>, keys: &[[f32; 3]]) -> AnimationSampler {
        AnimationSampler {
            input,
            interpolation: Interpolation::Linear,
            output: keys.iter().flatten().copied().collect(),
        }
    }

    #[test]
    fn linear_interpolates_and_clamps() {
        let s = linear_vec3(
            vec![1.0, 2.0, 4.0],
            &[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 20.0, 0.0]],
        );
        // Clamped before the first keyframe (input starts at t = 1).
        assert!(approx3(
            vec3(s.sample(TargetPath::Translation, 0.0).unwrap()),
            [0.0, 0.0, 0.0]
        ));
        // Exact keyframe hits use the stored value as-is.
        assert!(approx3(
            vec3(s.sample(TargetPath::Translation, 2.0).unwrap()),
            [10.0, 0.0, 0.0]
        ));
        // Midpoints of both segments.
        assert!(approx3(
            vec3(s.sample(TargetPath::Translation, 1.5).unwrap()),
            [5.0, 0.0, 0.0]
        ));
        assert!(approx3(
            vec3(s.sample(TargetPath::Translation, 3.0).unwrap()),
            [10.0, 10.0, 0.0]
        ));
        // Clamped after the last keyframe.
        assert!(approx3(
            vec3(s.sample(TargetPath::Translation, 99.0).unwrap()),
            [10.0, 20.0, 0.0]
        ));
    }

    #[test]
    fn step_holds_previous_keyframe() {
        let s = AnimationSampler {
            input: vec![0.0, 1.0],
            interpolation: Interpolation::Step,
            output: vec![1.0, 1.0, 1.0, 5.0, 5.0, 5.0],
        };
        assert!(approx3(
            vec3(s.sample(TargetPath::Scale, 0.999).unwrap()),
            [1.0, 1.0, 1.0]
        ));
        assert!(approx3(
            vec3(s.sample(TargetPath::Scale, 1.0).unwrap()),
            [5.0, 5.0, 5.0]
        ));
    }

    #[test]
    fn single_keyframe_holds_value_everywhere() {
        let s = linear_vec3(vec![3.0], &[[7.0, 8.0, 9.0]]);
        for t in [-1.0, 0.0, 3.0, 100.0] {
            assert!(approx3(
                vec3(s.sample(TargetPath::Translation, t).unwrap()),
                [7.0, 8.0, 9.0]
            ));
        }
    }

    #[test]
    fn linear_rotation_slerps_shortest_path() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let quarter = quat_from_axis_angle([0.0, 0.0, 1.0], FRAC_PI_2);
        let s = AnimationSampler {
            input: vec![0.0, 1.0],
            interpolation: Interpolation::Linear,
            output: id.iter().chain(quarter.iter()).copied().collect(),
        };
        let mid = quat(s.sample(TargetPath::Rotation, 0.5).unwrap());
        let expected = quat_from_axis_angle([0.0, 0.0, 1.0], FRAC_PI_2 / 2.0);
        for i in 0..4 {
            assert!(approx(mid[i], expected[i]), "{mid:?} vs {expected:?}");
        }
        // Unit length preserved.
        let len: f32 = mid.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(approx(len, 1.0));
    }

    #[test]
    fn cubic_spline_matches_hermite_closed_form() {
        // Scalar-style check on the X component: keyframes at t = 0
        // (v = 0, out-tangent b = 3) and t = 2 (v = 4, in-tangent
        // a = 0). At u = 0.5 (t_c = 1), t_d = 2:
        //   v_t = 0.5 * 0 + 2 * 0.125 * 3 + 0.5 * 4 + 0 = 2.75
        let s = AnimationSampler {
            input: vec![0.0, 2.0],
            interpolation: Interpolation::CubicSpline,
            output: vec![
                // key 0: a, v, b
                0.0, 0.0, 0.0, /* a0 */
                0.0, 0.0, 0.0, /* v0 */
                3.0, 0.0, 0.0, /* b0 */
                // key 1: a, v, b
                0.0, 0.0, 0.0, /* a1 */
                4.0, 0.0, 0.0, /* v1 */
                0.0, 0.0, 0.0, /* b1 */
            ],
        };
        let v = vec3(s.sample(TargetPath::Translation, 1.0).unwrap());
        assert!(approx(v[0], 2.75), "{v:?}");
        // Endpoints hit the stored values exactly; clamping uses the
        // value slot, not the tangents.
        assert!(approx3(
            vec3(s.sample(TargetPath::Translation, 0.0).unwrap()),
            [0.0, 0.0, 0.0]
        ));
        assert!(approx3(
            vec3(s.sample(TargetPath::Translation, 5.0).unwrap()),
            [4.0, 0.0, 0.0]
        ));
        // Zero tangents everywhere degrade to a smoothstep between
        // values: at u = 0.5 that is the midpoint.
        let smooth = AnimationSampler {
            input: vec![0.0, 2.0],
            interpolation: Interpolation::CubicSpline,
            output: vec![
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, //
                0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            ],
        };
        let v = vec3(smooth.sample(TargetPath::Translation, 1.0).unwrap());
        assert!(approx(v[0], 2.0), "{v:?}");
    }

    #[test]
    fn cubic_rotation_is_normalised() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let quarter = quat_from_axis_angle([0.0, 1.0, 0.0], FRAC_PI_2);
        let mut output = Vec::new();
        for q in [id, quarter] {
            output.extend_from_slice(&[0.0; 4]); // in-tangent
            output.extend_from_slice(&q); // value
            output.extend_from_slice(&[0.0; 4]); // out-tangent
        }
        let s = AnimationSampler {
            input: vec![0.0, 1.0],
            interpolation: Interpolation::CubicSpline,
            output,
        };
        let q = quat(s.sample(TargetPath::Rotation, 0.5).unwrap());
        let len: f32 = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(approx(len, 1.0), "{q:?} has length {len}");
    }

    #[test]
    fn malformed_samplers_return_none() {
        // Empty input.
        let s = AnimationSampler::default();
        assert_eq!(s.sample(TargetPath::Translation, 0.0), None);
        // Output length mismatch for the path.
        let s = linear_vec3(vec![0.0, 1.0], &[[0.0; 3]]);
        assert_eq!(s.sample(TargetPath::Translation, 0.5), None);
        // Cubic with a single keyframe.
        let s = AnimationSampler {
            input: vec![0.0],
            interpolation: Interpolation::CubicSpline,
            output: vec![0.0; 9],
        };
        assert_eq!(s.sample(TargetPath::Translation, 0.0), None);
    }

    /// Graph with one root and one child; both TRS.
    fn graph_two() -> (NodeGraph, usize, usize) {
        let mut g = NodeGraph::new();
        let child = g.push(SceneNode {
            name: "child".into(),
            transform: NodeTransform::from_translation([1.0, 0.0, 0.0]),
            children: vec![],
        });
        let root = g.push_root(SceneNode {
            name: "root".into(),
            transform: NodeTransform::IDENTITY,
            children: vec![child],
        });
        (g, root, child)
    }

    #[test]
    fn validate_catches_defects() {
        let (g, root, _child) = graph_two();
        let base_sampler = linear_vec3(vec![0.0, 1.0], &[[0.0; 3], [1.0, 0.0, 0.0]]);
        let base_channel = AnimationChannel {
            sampler: 0,
            target_node: root,
            path: TargetPath::Translation,
        };

        // Well-formed passes.
        let ok = NodeAnimation {
            name: "ok".into(),
            samplers: vec![base_sampler.clone()],
            channels: vec![base_channel],
        };
        assert_eq!(ok.validate(&g), Ok(()));

        // Sampler out of range.
        let mut a = ok.clone();
        a.channels[0].sampler = 5;
        assert_eq!(
            a.validate(&g),
            Err(NodeAnimationError::SamplerOutOfRange {
                channel: 0,
                sampler: 5
            })
        );

        // Node out of range.
        let mut a = ok.clone();
        a.channels[0].target_node = 99;
        assert_eq!(
            a.validate(&g),
            Err(NodeAnimationError::NodeOutOfRange {
                channel: 0,
                node: 99
            })
        );

        // Duplicate target.
        let mut a = ok.clone();
        a.channels.push(base_channel);
        assert_eq!(
            a.validate(&g),
            Err(NodeAnimationError::DuplicateTarget {
                node: root,
                path: TargetPath::Translation
            })
        );

        // Matrix node targeted.
        let mut g2 = g.clone();
        g2.nodes[root].transform = NodeTransform::Matrix(Mat4::IDENTITY);
        assert_eq!(
            ok.validate(&g2),
            Err(NodeAnimationError::MatrixNodeTargeted {
                channel: 0,
                node: root
            })
        );

        // Empty input.
        let mut a = ok.clone();
        a.samplers[0].input.clear();
        a.samplers[0].output.clear();
        assert_eq!(
            a.validate(&g),
            Err(NodeAnimationError::EmptyInput { sampler: 0 })
        );

        // Non-increasing input.
        let mut a = ok.clone();
        a.samplers[0].input = vec![1.0, 1.0];
        assert_eq!(
            a.validate(&g),
            Err(NodeAnimationError::NonIncreasingInput { sampler: 0 })
        );

        // Non-finite data.
        let mut a = ok.clone();
        a.samplers[0].output[0] = f32::NAN;
        assert_eq!(
            a.validate(&g),
            Err(NodeAnimationError::NonFiniteData { sampler: 0 })
        );

        // Output length mismatch (rotation needs 4 per key).
        let mut a = ok.clone();
        a.channels[0].path = TargetPath::Rotation;
        assert_eq!(
            a.validate(&g),
            Err(NodeAnimationError::OutputLengthMismatch {
                sampler: 0,
                expected: 8,
                got: 6
            })
        );

        // Cubic spline too short.
        let mut a = ok.clone();
        a.samplers[0] = AnimationSampler {
            input: vec![0.0],
            interpolation: Interpolation::CubicSpline,
            output: vec![0.0; 9],
        };
        assert_eq!(
            a.validate(&g),
            Err(NodeAnimationError::CubicSplineTooShort { sampler: 0 })
        );

        // Errors are std errors with useful Display.
        let e = NodeAnimationError::DuplicateTarget {
            node: 3,
            path: TargetPath::Scale,
        };
        assert!(e.to_string().contains('3'));
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn duration_is_last_referenced_keyframe() {
        let (g, root, child) = graph_two();
        let anim = NodeAnimation {
            name: String::new(),
            samplers: vec![
                linear_vec3(vec![0.0, 2.5], &[[0.0; 3], [1.0; 3]]),
                linear_vec3(vec![0.0, 4.0], &[[1.0; 3], [2.0; 3]]),
            ],
            channels: vec![
                AnimationChannel {
                    sampler: 0,
                    target_node: root,
                    path: TargetPath::Translation,
                },
                AnimationChannel {
                    sampler: 1,
                    target_node: child,
                    path: TargetPath::Scale,
                },
            ],
        };
        assert_eq!(anim.validate(&g), Ok(()));
        assert!(approx(anim.duration(), 4.0));
        assert!(approx(NodeAnimation::default().duration(), 0.0));
    }

    #[test]
    fn non_animated_properties_keep_base_values() {
        let (mut g, root, _child) = graph_two();
        g.nodes[root].transform = NodeTransform::Trs {
            translation: [0.0, 9.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
        };
        // Animate only the translation.
        let anim = NodeAnimation {
            name: String::new(),
            samplers: vec![linear_vec3(
                vec![0.0, 1.0],
                &[[0.0, 9.0, 0.0], [10.0, 9.0, 0.0]],
            )],
            channels: vec![AnimationChannel {
                sampler: 0,
                target_node: root,
                path: TargetPath::Translation,
            }],
        };
        let locals = anim.local_transforms_at(&g, 0.5);
        let NodeTransform::Trs {
            translation,
            rotation,
            scale,
        } = locals[root]
        else {
            panic!("expected TRS");
        };
        assert!(approx3(translation, [5.0, 9.0, 0.0]));
        assert_eq!(rotation, [0.0, 0.0, 0.0, 1.0]);
        assert!(approx3(scale, [2.0, 2.0, 2.0])); // base scale kept
    }

    #[test]
    fn global_matrices_at_composes_animated_parent_chain() {
        let (g, root, child) = graph_two();
        // Rotate the root 90° about +Z over one second; the child sits
        // at +X 1 in root space, so at t = 1 its world origin is at
        // (0, 1, 0), and at t = 0 at (1, 0, 0).
        let id = [0.0, 0.0, 0.0, 1.0];
        let quarter = quat_from_axis_angle([0.0, 0.0, 1.0], FRAC_PI_2);
        let anim = NodeAnimation {
            name: "spin".into(),
            samplers: vec![AnimationSampler {
                input: vec![0.0, 1.0],
                interpolation: Interpolation::Linear,
                output: id.iter().chain(quarter.iter()).copied().collect(),
            }],
            channels: vec![AnimationChannel {
                sampler: 0,
                target_node: root,
                path: TargetPath::Rotation,
            }],
        };
        assert_eq!(anim.validate(&g), Ok(()));

        let at = |t: f32| {
            anim.global_matrices_at(&g, t)[child]
                .unwrap()
                .transform_point([0.0, 0.0, 0.0])
        };
        assert!(approx3(at(0.0), [1.0, 0.0, 0.0]), "{:?}", at(0.0));
        assert!(approx3(at(1.0), [0.0, 1.0, 0.0]), "{:?}", at(1.0));
        // Halfway: 45° about +Z.
        let sqrt_half = (0.5f32).sqrt();
        assert!(
            approx3(at(0.5), [sqrt_half, sqrt_half, 0.0]),
            "{:?}",
            at(0.5)
        );
        // Past the end: clamped to the last keyframe.
        assert!(approx3(at(9.0), [0.0, 1.0, 0.0]));
    }
}
