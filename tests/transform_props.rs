//! Property tests for [`Transform`] matrix lowering + bounding-box
//! computation.
//!
//! These exercise the high-level `Transform` → `oxideav_core::Transform2D`
//! lowering (`to_matrix`), point mapping (`apply_to_point`), and the
//! axis-aligned `bbox` over thousands of pseudo-randomly generated
//! transforms. No external proptest dependency — a small deterministic
//! xorshift generator drives the cases so failures reproduce exactly
//! from the printed seed.

use oxideav_core::Point;
use oxideav_scene::Transform;

/// Deterministic xorshift64* PRNG. Seeded per test so any failure is
/// reproducible from the literal seed in the source.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Avoid the zero fixed point.
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F491_4F6CDD1D)
    }

    /// Uniform f32 in `[lo, hi)`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let u = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32; // [0,1)
        lo + u * (hi - lo)
    }
}

/// A reasonable but wide-ranging random transform + content box.
fn rand_case(rng: &mut Rng) -> (Transform, f32, f32) {
    let t = Transform {
        position: (rng.range(-500.0, 500.0), rng.range(-500.0, 500.0)),
        // Keep scales away from exact zero so areas stay meaningful;
        // include negatives to cover flips.
        scale: (rng.range(-4.0, 4.0), rng.range(-4.0, 4.0)),
        rotation: rng.range(-6.5, 6.5),
        anchor: (rng.range(0.0, 1.0), rng.range(0.0, 1.0)),
        skew: (rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)),
    };
    let w = rng.range(1.0, 800.0);
    let h = rng.range(1.0, 800.0);
    (t, w, h)
}

fn approx(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps * (1.0 + a.abs().max(b.abs()))
}

#[test]
fn identity_is_a_no_op_for_every_content_size() {
    let mut rng = Rng::new(0xA11CE);
    for _ in 0..2000 {
        let w = rng.range(0.0, 1000.0);
        let h = rng.range(0.0, 1000.0);
        let m = Transform::identity().to_matrix(w, h);
        assert!(
            m.is_identity(),
            "identity over {w}x{h} should stay identity"
        );
    }
}

#[test]
fn apply_to_point_matches_to_matrix_apply() {
    // `apply_to_point` is documented as sugar over `to_matrix().apply()`;
    // they must never disagree.
    let mut rng = Rng::new(0xBADF00D);
    for _ in 0..3000 {
        let (t, w, h) = rand_case(&mut rng);
        let px = rng.range(-1000.0, 1000.0);
        let py = rng.range(-1000.0, 1000.0);
        let p = Point::new(px, py);
        let via_helper = t.apply_to_point(w, h, p);
        let via_matrix = t.to_matrix(w, h).apply(p);
        assert_eq!(via_helper, via_matrix);
    }
}

#[test]
fn pure_translation_preserves_shape_and_area() {
    // Translation only: the bbox is the content box shifted by
    // `position`, so its extent equals the content extent exactly and
    // is anchor-independent.
    let mut rng = Rng::new(0xC0FFEE);
    for _ in 0..2000 {
        let dx = rng.range(-500.0, 500.0);
        let dy = rng.range(-500.0, 500.0);
        let ax = rng.range(0.0, 1.0);
        let ay = rng.range(0.0, 1.0);
        let w = rng.range(1.0, 600.0);
        let h = rng.range(1.0, 600.0);
        let t = Transform {
            position: (dx, dy),
            anchor: (ax, ay),
            ..Transform::identity()
        };
        let bb = t.bbox(w, h);
        assert!(approx(bb.width, w, 1e-4), "width drift under translation");
        assert!(approx(bb.height, h, 1e-4), "height drift under translation");
        assert!(approx(bb.x, dx, 1e-4), "x not shifted by position");
        assert!(approx(bb.y, dy, 1e-4), "y not shifted by position");
    }
}

#[test]
fn bbox_extent_is_always_non_negative_and_finite() {
    let mut rng = Rng::new(0xD15EA5E);
    for i in 0..5000 {
        let (t, w, h) = rand_case(&mut rng);
        let bb = t.bbox(w, h);
        assert!(
            bb.width >= 0.0 && bb.height >= 0.0,
            "case {i}: negative extent {}x{}",
            bb.width,
            bb.height
        );
        assert!(
            bb.x.is_finite() && bb.y.is_finite() && bb.width.is_finite() && bb.height.is_finite(),
            "case {i}: non-finite bbox"
        );
    }
}

#[test]
fn bbox_contains_all_four_transformed_corners() {
    // Core invariant of an AABB: every mapped corner must lie inside
    // (or on) the reported box.
    let mut rng = Rng::new(0xFEEDFACE);
    for i in 0..5000 {
        let (t, w, h) = rand_case(&mut rng);
        let m = t.to_matrix(w, h);
        let bb = t.bbox(w, h);
        let corners = [
            m.apply(Point::new(0.0, 0.0)),
            m.apply(Point::new(w, 0.0)),
            m.apply(Point::new(w, h)),
            m.apply(Point::new(0.0, h)),
        ];
        // Slack proportional to the box size to absorb f32 rounding in
        // the min/max reduction.
        let sx = 1e-3 * (1.0 + bb.width.abs());
        let sy = 1e-3 * (1.0 + bb.height.abs());
        for (j, c) in corners.iter().enumerate() {
            assert!(
                c.x >= bb.x - sx && c.x <= bb.x + bb.width + sx,
                "case {i} corner {j}: x {} outside [{},{}]",
                c.x,
                bb.x,
                bb.x + bb.width
            );
            assert!(
                c.y >= bb.y - sy && c.y <= bb.y + bb.height + sy,
                "case {i} corner {j}: y {} outside [{},{}]",
                c.y,
                bb.y,
                bb.y + bb.height
            );
        }
    }
}

#[test]
fn bbox_is_tight_on_at_least_one_corner_per_edge() {
    // For an axis-aligned hull of 4 points, each of the four box edges
    // must be touched by at least one mapped corner — otherwise the box
    // is looser than the data warrants.
    let mut rng = Rng::new(0x5EED1);
    for i in 0..4000 {
        let (t, w, h) = rand_case(&mut rng);
        let m = t.to_matrix(w, h);
        let bb = t.bbox(w, h);
        let corners = [
            m.apply(Point::new(0.0, 0.0)),
            m.apply(Point::new(w, 0.0)),
            m.apply(Point::new(w, h)),
            m.apply(Point::new(0.0, h)),
        ];
        let sx = 1e-3 * (1.0 + bb.width.abs());
        let sy = 1e-3 * (1.0 + bb.height.abs());
        let touches_min_x = corners.iter().any(|c| (c.x - bb.x).abs() <= sx);
        let touches_max_x = corners
            .iter()
            .any(|c| (c.x - (bb.x + bb.width)).abs() <= sx);
        let touches_min_y = corners.iter().any(|c| (c.y - bb.y).abs() <= sy);
        let touches_max_y = corners
            .iter()
            .any(|c| (c.y - (bb.y + bb.height)).abs() <= sy);
        assert!(
            touches_min_x && touches_max_x && touches_min_y && touches_max_y,
            "case {i}: bbox not tight against its corners"
        );
    }
}

#[test]
fn rotation_preserves_bbox_area_lower_bound() {
    // Rotation about the anchor can only grow (never shrink) the AABB
    // of a rectangle: the rotated rect's area is preserved, and an AABB
    // enclosing it has area >= that. Compare against the unrotated
    // (scale-only) extent's area to confirm the box never collapses.
    let mut rng = Rng::new(0x12345);
    for i in 0..3000 {
        let sx = rng.range(0.2, 3.0); // positive to keep area sign simple
        let sy = rng.range(0.2, 3.0);
        let w = rng.range(2.0, 400.0);
        let h = rng.range(2.0, 400.0);
        let rot = rng.range(-6.5, 6.5);
        let scaled_only = Transform {
            scale: (sx, sy),
            ..Transform::identity()
        };
        let rotated = Transform {
            scale: (sx, sy),
            rotation: rot,
            ..Transform::identity()
        };
        let area_rect = (sx * w) * (sy * h); // area of the scaled rect itself
        let bb = rotated.bbox(w, h);
        let bb_area = bb.width * bb.height;
        // The AABB of the rotated rect must enclose the rect's true area.
        assert!(
            bb_area >= area_rect - 1e-2 * (1.0 + area_rect),
            "case {i}: rotated AABB area {bb_area} < rect area {area_rect}"
        );
        // Sanity: the unrotated (axis-aligned) box reports exactly the
        // scaled extent.
        let bb0 = scaled_only.bbox(w, h);
        assert!(
            approx(bb0.width * bb0.height, area_rect, 1e-3),
            "case {i}: scale-only area mismatch"
        );
    }
}

#[test]
fn translation_commutes_with_position_field() {
    // Adding `(dx, dy)` to `position` shifts every mapped point by
    // exactly `(dx, dy)` — translation is applied last and unscaled.
    let mut rng = Rng::new(0x99AA);
    for _ in 0..3000 {
        let (mut t, w, h) = rand_case(&mut rng);
        let p = Point::new(rng.range(-200.0, 200.0), rng.range(-200.0, 200.0));
        let before = t.apply_to_point(w, h, p);
        let dx = rng.range(-300.0, 300.0);
        let dy = rng.range(-300.0, 300.0);
        t.position = (t.position.0 + dx, t.position.1 + dy);
        let after = t.apply_to_point(w, h, p);
        assert!(approx(after.x - before.x, dx, 1e-3), "x shift mismatch");
        assert!(approx(after.y - before.y, dy, 1e-3), "y shift mismatch");
    }
}
