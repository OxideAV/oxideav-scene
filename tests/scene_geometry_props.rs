//! Property tests for the scene-level geometry queries: per-object
//! [`SceneObject::bbox`], the scene-wide union
//! [`Scene::bbox_at`], and the AABB hit test
//! [`Scene::hit_test_at`].
//!
//! Same deterministic xorshift64* generator as
//! `tests/transform_props.rs` — failures reproduce from the literal
//! seed printed in each test source.

use oxideav_core::Point;
use oxideav_scene::{
    ClipRect, Lifetime, ObjectId, ObjectKind, Scene, SceneObject, Shape, Transform,
};

/// Deterministic xorshift64* PRNG.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
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

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let u = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        lo + u * (hi - lo)
    }

    fn range_i(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u64() % ((hi - lo) as u64)) as i32
    }
}

/// Random axis-aligned rect-shape SceneObject with a translation
/// transform — keeps the AABB tight to the declared geometry so the
/// tests can reason about it in closed form.
fn rand_rect_object(rng: &mut Rng, id: u64) -> (SceneObject, f32, f32, f32, f32) {
    let w = rng.range(1.0, 100.0);
    let h = rng.range(1.0, 100.0);
    let x = rng.range(-500.0, 500.0);
    let y = rng.range(-500.0, 500.0);
    let z = rng.range_i(-10, 10);
    let obj = SceneObject {
        id: ObjectId::new(id),
        kind: ObjectKind::Shape(Shape::Rect {
            width: w,
            height: h,
            fill: 0,
            stroke: None,
            corner_radius: 0.0,
        }),
        transform: Transform {
            position: (x, y),
            ..Transform::identity()
        },
        z_order: z,
        ..SceneObject::default()
    };
    (obj, x, y, w, h)
}

#[test]
fn empty_scene_bbox_is_none_for_any_time() {
    let mut rng = Rng::new(0xE111);
    let s = Scene::default();
    for _ in 0..200 {
        // i64 in a wide but legal range.
        let t = (rng.next_u64() as i64) >> 8;
        assert!(s
            .bbox_at(t, (rng.range(0.0, 1000.0), rng.range(0.0, 1000.0)))
            .is_none());
    }
}

#[test]
fn single_object_scene_bbox_equals_object_bbox() {
    let mut rng = Rng::new(0x511C);
    for _ in 0..500 {
        let mut s = Scene::default();
        let (obj, x, y, w, h) = rand_rect_object(&mut rng, 1);
        s.objects.push(obj);
        let bb = s.bbox_at(0, (0.0, 0.0)).expect("one live object");
        assert!((bb.x - x).abs() < 1e-3);
        assert!((bb.y - y).abs() < 1e-3);
        assert!((bb.width - w).abs() < 1e-3);
        assert!((bb.height - h).abs() < 1e-3);
    }
}

#[test]
fn union_bbox_contains_every_member_bbox() {
    let mut rng = Rng::new(0x5C11E);
    for _ in 0..400 {
        let mut s = Scene::default();
        let n = rng.range_i(2, 8);
        let mut members = Vec::new();
        for i in 0..n as u64 {
            let (o, x, y, w, h) = rand_rect_object(&mut rng, i + 1);
            members.push((x, y, w, h));
            s.objects.push(o);
        }
        let bb = s.bbox_at(0, (0.0, 0.0)).expect("scene non-empty");
        // Slack proportional to the union size.
        let sx = 1e-3 * (1.0 + bb.width.abs());
        let sy = 1e-3 * (1.0 + bb.height.abs());
        for (x, y, w, h) in members {
            assert!(
                x >= bb.x - sx,
                "union x={} doesn't cover member x={}",
                bb.x,
                x
            );
            assert!(
                y >= bb.y - sy,
                "union y={} doesn't cover member y={}",
                bb.y,
                y
            );
            assert!(
                x + w <= bb.x + bb.width + sx,
                "union right doesn't cover member right"
            );
            assert!(
                y + h <= bb.y + bb.height + sy,
                "union bottom doesn't cover member bottom"
            );
        }
    }
}

#[test]
fn union_bbox_drops_dead_objects() {
    // Build a scene with one always-live object + N dead objects far
    // away. bbox_at on a timestamp the dead objects don't span should
    // equal the live object's bbox alone.
    let mut rng = Rng::new(0xDEAD);
    for _ in 0..200 {
        let mut s = Scene {
            duration: oxideav_scene::SceneDuration::Finite(1_000),
            ..Scene::default()
        };
        let (live, lx, ly, lw, lh) = rand_rect_object(&mut rng, 1);
        s.objects.push(live);
        // Append a few "dead" objects with disjoint lifetimes far from
        // the live one's footprint.
        for i in 2..6u64 {
            let mut dead = SceneObject {
                id: ObjectId::new(i),
                kind: ObjectKind::Shape(Shape::Rect {
                    width: 50.0,
                    height: 50.0,
                    fill: 0,
                    stroke: None,
                    corner_radius: 0.0,
                }),
                transform: Transform {
                    position: (10_000.0, 10_000.0),
                    ..Transform::identity()
                },
                ..SceneObject::default()
            };
            dead.lifetime = Lifetime {
                start: 500,
                end: Some(900),
            };
            s.objects.push(dead);
        }
        let bb = s.bbox_at(10, (0.0, 0.0)).expect("live object exists");
        // If a dead object had leaked in, max_x/max_y would have run
        // to 10050 — verify it didn't.
        assert!((bb.x - lx).abs() < 1e-3);
        assert!((bb.y - ly).abs() < 1e-3);
        assert!((bb.width - lw).abs() < 1e-3);
        assert!((bb.height - lh).abs() < 1e-3);
    }
}

#[test]
fn hit_test_top_z_order_wins() {
    let mut rng = Rng::new(0x101);
    for _ in 0..500 {
        let mut s = Scene::default();
        // Stack `n` overlapping rects at random z-orders. The highest
        // z-order should be the hit.
        let n = rng.range_i(2, 6) as u64;
        let mut top_id: ObjectId = ObjectId::new(0);
        let mut top_z = i32::MIN;
        let mut top_idx: i64 = -1;
        for i in 0..n {
            let obj = SceneObject {
                id: ObjectId::new(i + 1),
                kind: ObjectKind::Shape(Shape::Rect {
                    width: 100.0,
                    height: 100.0,
                    fill: 0,
                    stroke: None,
                    corner_radius: 0.0,
                }),
                transform: Transform {
                    position: (0.0, 0.0),
                    ..Transform::identity()
                },
                z_order: rng.range_i(-5, 5),
                ..SceneObject::default()
            };
            // Tie-break: equal z + later index wins.
            if obj.z_order > top_z || (obj.z_order == top_z && (i as i64) > top_idx) {
                top_z = obj.z_order;
                top_id = obj.id;
                top_idx = i as i64;
            }
            s.objects.push(obj);
        }
        let hit = s.hit_test_at(0, Point::new(50.0, 50.0), (0.0, 0.0));
        assert_eq!(hit, Some(top_id));
    }
}

#[test]
fn hit_test_misses_outside_every_object() {
    let mut rng = Rng::new(0xCAFE);
    for _ in 0..400 {
        let mut s = Scene::default();
        // Place objects in the lower-left quadrant; query a point in
        // the far upper-right.
        for i in 0..5u64 {
            let (o, _, _, _, _) = rand_rect_object(&mut rng, i + 1);
            // Force each object's footprint into a bounded region.
            let mut clamped = o;
            clamped.transform.position = (rng.range(-100.0, 0.0), rng.range(-100.0, 0.0));
            s.objects.push(clamped);
        }
        let hit = s.hit_test_at(0, Point::new(10_000.0, 10_000.0), (0.0, 0.0));
        assert!(hit.is_none());
    }
}

#[test]
fn clip_collapses_union_contribution() {
    // An object whose clip rect doesn't overlap its transformed bbox
    // must not stretch the scene-wide union.
    let mut rng = Rng::new(0xC11);
    for _ in 0..300 {
        let mut s = Scene::default();
        let (anchor, ax, ay, aw, ah) = rand_rect_object(&mut rng, 1);
        s.objects.push(anchor);
        let mut bad = SceneObject {
            id: ObjectId::new(2),
            kind: ObjectKind::Shape(Shape::Rect {
                width: 5_000.0,
                height: 5_000.0,
                fill: 0,
                stroke: None,
                corner_radius: 0.0,
            }),
            transform: Transform {
                position: (2_000.0, 2_000.0),
                ..Transform::identity()
            },
            ..SceneObject::default()
        };
        // Clip far away from `bad`'s actual position.
        bad.clip = Some(ClipRect {
            x: -10_000.0,
            y: -10_000.0,
            width: 1.0,
            height: 1.0,
        });
        s.objects.push(bad);
        let bb = s.bbox_at(0, (0.0, 0.0)).expect("anchor live");
        // Union must equal the anchor's bbox alone — the bad object
        // is clipped out.
        assert!((bb.x - ax).abs() < 1e-3);
        assert!((bb.y - ay).abs() < 1e-3);
        assert!((bb.width - aw).abs() < 1e-3);
        assert!((bb.height - ah).abs() < 1e-3);
    }
}
