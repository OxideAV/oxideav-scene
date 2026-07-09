//! Property-style tests for the 3D node surface: random (but
//! deterministic) TRS transforms, trees, and animations checked
//! against closed-form invariants.

use oxideav_scene::node::{
    quat_dot, quat_normalize, quat_slerp, Mat4, NodeGraph, NodeTransform, SceneNode,
};
use oxideav_scene::node_animation::{
    AnimationChannel, AnimationSampler, Interpolation, NodeAnimation, SampledValue, TargetPath,
};

/// Deterministic xorshift32 so failures reproduce.
struct Rng(u32);

impl Rng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform-ish float in `[lo, hi)`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let u = (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32;
        lo + u * (hi - lo)
    }

    fn unit_quat(&mut self) -> [f32; 4] {
        quat_normalize([
            self.range(-1.0, 1.0),
            self.range(-1.0, 1.0),
            self.range(-1.0, 1.0),
            self.range(-1.0, 1.0),
        ])
    }

    fn trs(&mut self) -> NodeTransform {
        NodeTransform::Trs {
            translation: [
                self.range(-50.0, 50.0),
                self.range(-50.0, 50.0),
                self.range(-50.0, 50.0),
            ],
            rotation: self.unit_quat(),
            scale: [
                self.range(0.2, 4.0),
                self.range(0.2, 4.0),
                self.range(0.2, 4.0),
            ],
        }
    }
}

fn assert_mat_approx(a: &Mat4, b: &Mat4, tol: f32, ctx: &str) {
    for i in 0..16 {
        assert!(
            (a.elements[i] - b.elements[i]).abs() < tol,
            "{ctx}: element {i}: {} vs {}",
            a.elements[i],
            b.elements[i]
        );
    }
}

#[test]
fn decompose_recompose_round_trips_random_trs() {
    let mut rng = Rng(0xA5A5_0001);
    for iter in 0..300 {
        let trs = rng.trs();
        let m = trs.local_matrix();
        let (t, q, s) = m
            .decompose_trs()
            .unwrap_or_else(|| panic!("iteration {iter}: decompose failed for {trs:?}"));
        let recomposed = NodeTransform::Trs {
            translation: t,
            rotation: q,
            scale: s,
        }
        .local_matrix();
        assert_mat_approx(&recomposed, &m, 2e-3, &format!("iteration {iter}"));
    }
}

#[test]
fn inverse_times_matrix_is_identity_for_random_trs() {
    let mut rng = Rng(0xA5A5_0002);
    for iter in 0..300 {
        let m = rng.trs().local_matrix();
        let inv = m
            .inverse()
            .unwrap_or_else(|| panic!("iteration {iter}: inverse failed"));
        let prod = m.mul(&inv);
        assert_mat_approx(&prod, &Mat4::IDENTITY, 2e-3, &format!("iteration {iter}"));
        // Determinant multiplies: det(M) * det(M^-1) == 1.
        let dd = m.determinant() * inv.determinant();
        assert!((dd - 1.0).abs() < 2e-3, "iteration {iter}: {dd}");
    }
}

#[test]
fn random_tree_global_matrix_matches_parent_chain_product() {
    let mut rng = Rng(0xA5A5_0003);
    for iter in 0..40 {
        // Random tree rooted at node 0: parent(i) < i.
        let n = 2 + (rng.next_u32() as usize % 14);
        let mut g = NodeGraph::new();
        let mut parent_of = vec![usize::MAX; n];
        for (i, slot) in parent_of.iter_mut().enumerate() {
            g.push(SceneNode {
                name: format!("n{i}"),
                transform: rng.trs(),
                children: vec![],
            });
            if i > 0 {
                let p = (rng.next_u32() as usize) % i;
                *slot = p;
                g.nodes[p].children.push(i);
            }
        }
        g.roots.push(0);
        assert_eq!(g.validate(), Ok(()), "iteration {iter}");

        let globals = g.global_matrices();
        for i in 0..n {
            // Manual parent-chain product, top down.
            let mut chain = vec![i];
            let mut cur = i;
            while parent_of[cur] != usize::MAX {
                cur = parent_of[cur];
                chain.push(cur);
            }
            chain.reverse();
            let mut m = Mat4::IDENTITY;
            for &idx in &chain {
                m = m.mul(&g.nodes[idx].local_matrix());
            }
            let got = globals[i].unwrap_or_else(|| panic!("iteration {iter}: node {i} orphaned"));
            assert_mat_approx(&got, &m, 5e-2, &format!("iteration {iter} node {i}"));
            // Spec rule restated: global(i) == global(parent) * local(i).
            if parent_of[i] != usize::MAX {
                let via_parent = globals[parent_of[i]]
                    .unwrap()
                    .mul(&g.nodes[i].local_matrix());
                assert_mat_approx(
                    &got,
                    &via_parent,
                    5e-2,
                    &format!("iteration {iter} node {i} via parent"),
                );
            }
        }
    }
}

#[test]
fn slerp_output_is_unit_and_hits_endpoints() {
    let mut rng = Rng(0xA5A5_0004);
    for iter in 0..300 {
        let a = rng.unit_quat();
        let b = rng.unit_quat();
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let s = quat_slerp(a, b, t);
            let len = quat_dot(s, s).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "iteration {iter} t={t}: {len}");
        }
        // Endpoints reproduce the inputs up to sign (same rotation).
        let s0 = quat_slerp(a, b, 0.0);
        let s1 = quat_slerp(a, b, 1.0);
        assert!(quat_dot(s0, a).abs() > 1.0 - 1e-4, "iteration {iter}");
        assert!(quat_dot(s1, b).abs() > 1.0 - 1e-4, "iteration {iter}");
    }
}

#[test]
fn linear_sampler_is_exact_at_keyframes_and_bounded_between() {
    let mut rng = Rng(0xA5A5_0005);
    for iter in 0..100 {
        // Random strictly-increasing input + random keys.
        let n = 2 + (rng.next_u32() as usize % 6);
        let mut input = Vec::with_capacity(n);
        let mut t = rng.range(-5.0, 5.0);
        for _ in 0..n {
            input.push(t);
            t += rng.range(0.1, 3.0);
        }
        let keys: Vec<[f32; 3]> = (0..n)
            .map(|_| {
                [
                    rng.range(-10.0, 10.0),
                    rng.range(-10.0, 10.0),
                    rng.range(-10.0, 10.0),
                ]
            })
            .collect();
        let sampler = AnimationSampler {
            input: input.clone(),
            interpolation: Interpolation::Linear,
            output: keys.iter().flatten().copied().collect(),
        };
        // Exact keyframe hits.
        for (k, &tk) in input.iter().enumerate() {
            let SampledValue::Vec3(v) = sampler.sample(TargetPath::Translation, tk).unwrap() else {
                panic!("expected Vec3");
            };
            for c in 0..3 {
                assert!(
                    (v[c] - keys[k][c]).abs() < 1e-5,
                    "iteration {iter} key {k}: {v:?} vs {:?}",
                    keys[k]
                );
            }
        }
        // Between two keyframes each component stays within the
        // segment's endpoint bounds (linear interpolation property).
        for k in 0..n - 1 {
            let mid = (input[k] + input[k + 1]) / 2.0;
            let SampledValue::Vec3(v) = sampler.sample(TargetPath::Translation, mid).unwrap()
            else {
                panic!("expected Vec3");
            };
            for c in 0..3 {
                let (lo, hi) = if keys[k][c] <= keys[k + 1][c] {
                    (keys[k][c], keys[k + 1][c])
                } else {
                    (keys[k + 1][c], keys[k][c])
                };
                assert!(
                    v[c] >= lo - 1e-4 && v[c] <= hi + 1e-4,
                    "iteration {iter} segment {k} component {c}: {} not in [{lo}, {hi}]",
                    v[c]
                );
            }
        }
    }
}

#[test]
fn step_and_linear_agree_with_cubic_zero_tangents_at_keyframes() {
    // At exact keyframe timestamps every interpolation mode must
    // return the stored value as-is (Appendix C.1 rule).
    let input = vec![0.0, 1.0, 2.5];
    let keys = [[1.0, 2.0, 3.0], [-4.0, 0.5, 6.0], [7.0, -8.0, 9.0]];
    let flat: Vec<f32> = keys.iter().flatten().copied().collect();
    let mut cubic_out = Vec::new();
    for k in &keys {
        cubic_out.extend_from_slice(&[0.0; 3]);
        cubic_out.extend_from_slice(k);
        cubic_out.extend_from_slice(&[0.0; 3]);
    }
    let samplers = [
        AnimationSampler {
            input: input.clone(),
            interpolation: Interpolation::Step,
            output: flat.clone(),
        },
        AnimationSampler {
            input: input.clone(),
            interpolation: Interpolation::Linear,
            output: flat,
        },
        AnimationSampler {
            input: input.clone(),
            interpolation: Interpolation::CubicSpline,
            output: cubic_out,
        },
    ];
    for (si, s) in samplers.iter().enumerate() {
        for (k, &tk) in input.iter().enumerate() {
            let SampledValue::Vec3(v) = s.sample(TargetPath::Translation, tk).unwrap() else {
                panic!("expected Vec3");
            };
            for c in 0..3 {
                assert!(
                    (v[c] - keys[k][c]).abs() < 1e-6,
                    "sampler {si} key {k}: {v:?} vs {:?}",
                    keys[k]
                );
            }
        }
    }
}

#[test]
fn animated_pose_equals_manually_posed_graph() {
    // Sampling an animation then composing must equal building the
    // graph with the sampled transform baked in.
    let mut rng = Rng(0xA5A5_0006);
    for iter in 0..50 {
        let mut g = NodeGraph::new();
        let child = g.push(SceneNode {
            name: "child".into(),
            transform: rng.trs(),
            children: vec![],
        });
        let root = g.push_root(SceneNode {
            name: "root".into(),
            transform: rng.trs(),
            children: vec![child],
        });

        let q0 = rng.unit_quat();
        let q1 = rng.unit_quat();
        let anim = NodeAnimation {
            name: String::new(),
            samplers: vec![AnimationSampler {
                input: vec![0.0, 1.0],
                interpolation: Interpolation::Linear,
                output: q0.iter().chain(q1.iter()).copied().collect(),
            }],
            channels: vec![AnimationChannel {
                sampler: 0,
                target_node: root,
                path: TargetPath::Rotation,
            }],
        };
        assert_eq!(anim.validate(&g), Ok(()));

        let t = rng.range(0.0, 1.0);
        let posed = anim.global_matrices_at(&g, t);

        // Bake the sampled rotation into a copy of the graph.
        let sampled = quat_slerp(q0, q1, t);
        let mut baked = g.clone();
        let NodeTransform::Trs {
            translation, scale, ..
        } = baked.nodes[root].transform
        else {
            panic!("expected TRS root");
        };
        baked.nodes[root].transform = NodeTransform::Trs {
            translation,
            rotation: sampled,
            scale,
        };
        let expected = baked.global_matrices();
        for idx in [root, child] {
            assert_mat_approx(
                &posed[idx].unwrap(),
                &expected[idx].unwrap(),
                1e-3,
                &format!("iteration {iter} node {idx}"),
            );
        }
    }
}
