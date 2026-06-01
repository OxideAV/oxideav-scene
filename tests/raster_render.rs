//! End-to-end tests for [`RasterRenderer`] through the public crate
//! surface — compositing backgrounds + shapes into an RGBA frame, and
//! driving the renderer over a finite scene via `RenderedSource`.

use oxideav_scene::{
    Background, Canvas, ClipRect, ObjectId, ObjectKind, RasterRenderer, Scene, SceneObject,
    SceneRenderer, Shape, Transform,
};

fn pixel(data: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let off = (y as usize * w as usize + x as usize) * 4;
    [data[off], data[off + 1], data[off + 2], data[off + 3]]
}

fn top_left_rect(id: u64, x: f32, y: f32, w: f32, h: f32, fill: u32, z: i32) -> SceneObject {
    SceneObject {
        id: ObjectId::new(id),
        kind: ObjectKind::Shape(Shape::Rect {
            width: w,
            height: h,
            fill,
            stroke: None,
            corner_radius: 0.0,
        }),
        transform: Transform {
            position: (x, y),
            anchor: (0.0, 0.0),
            ..Transform::identity()
        },
        z_order: z,
        ..SceneObject::default()
    }
}

#[test]
fn composites_background_and_two_shapes() {
    let mut scene = Scene {
        canvas: Canvas::raster(64, 64),
        background: Background::Solid(0x202020FF),
        ..Scene::default()
    };
    // Blue panel (z=0), red badge on top (z=1), overlapping.
    scene
        .objects
        .push(top_left_rect(1, 0.0, 0.0, 40.0, 40.0, 0x0000FFFF, 0));
    scene
        .objects
        .push(top_left_rect(2, 10.0, 10.0, 20.0, 20.0, 0xFF0000FF, 1));

    let mut r = RasterRenderer::new();
    r.prepare(&scene).unwrap();
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    let data = &frame.planes[0].data;
    assert_eq!(data.len(), 64 * 64 * 4);

    // Background corner (60,60): dark grey backdrop.
    assert_eq!(pixel(data, 64, 60, 60), [0x20, 0x20, 0x20, 0xFF]);
    // Blue-only region (35,5): blue, not red.
    let blue = pixel(data, 64, 35, 5);
    assert!(blue[2] > 200 && blue[0] < 60, "expected blue: {blue:?}");
    // Overlap region (15,15): red badge wins.
    let red = pixel(data, 64, 15, 15);
    assert!(red[0] > 200 && red[2] < 60, "expected red on top: {red:?}");
}

#[test]
fn drives_a_finite_scene_to_frames() {
    use oxideav_core::Rational;
    use oxideav_scene::{RenderedSource, SceneSource};

    let mut scene = Scene {
        canvas: Canvas::raster(16, 16),
        duration: oxideav_scene::SceneDuration::Finite(100),
        framerate: Rational::new(30, 1),
        background: Background::Solid(0x0000FFFF),
        ..Scene::default()
    };
    scene
        .objects
        .push(top_left_rect(1, 0.0, 0.0, 16.0, 16.0, 0xFFFFFFFF, 0));

    let mut src = RenderedSource::new(scene, RasterRenderer::new());
    // Pull the first frame; it should carry a real RGBA buffer.
    let first = src
        .pull()
        .expect("pull should succeed")
        .expect("source should yield a first frame");
    let frame = first.video.expect("frame should carry video");
    assert_eq!(frame.planes[0].data.len(), 16 * 16 * 4);
    // The white rect fills the canvas → centre pixel is opaque white.
    let px = pixel(&frame.planes[0].data, 16, 8, 8);
    assert_eq!(px, [0xFF, 0xFF, 0xFF, 0xFF]);
}

#[test]
fn group_composes_children_under_group_transform() {
    // Two child rects inside a group; the group is translated +20, +20.
    // Each child rect should land at (group_pos + child_pos).
    let parent = SceneObject {
        id: ObjectId::new(100),
        kind: ObjectKind::Group(vec![ObjectId::new(1), ObjectId::new(2)]),
        transform: Transform {
            position: (20.0, 20.0),
            anchor: (0.0, 0.0),
            ..Transform::identity()
        },
        ..SceneObject::default()
    };
    let child1 = top_left_rect(1, 0.0, 0.0, 8.0, 8.0, 0xFF0000FF, 0);
    let child2 = top_left_rect(2, 20.0, 0.0, 8.0, 8.0, 0x00FF00FF, 0);

    let mut scene = Scene {
        canvas: Canvas::raster(64, 64),
        background: Background::Transparent,
        ..Scene::default()
    };
    scene.objects.push(parent);
    scene.objects.push(child1);
    scene.objects.push(child2);

    let mut r = RasterRenderer::new();
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    let data = &frame.planes[0].data;

    // Red child at group(20,20) + child(0,0) → (20..28, 20..28).
    let red = pixel(data, 64, 23, 23);
    assert!(red[0] > 200 && red[1] < 60, "expected red: {red:?}");

    // Green child at group(20,20) + child(20,0) → (40..48, 20..28).
    let green = pixel(data, 64, 43, 23);
    assert!(green[1] > 200 && green[0] < 60, "expected green: {green:?}");

    // Children should NOT also paint at their bare top-level positions
    // (the group is the only consumer of them). The bare red position
    // (0..8, 0..8) should be clear.
    assert_eq!(pixel(data, 64, 4, 4)[3], 0, "child must not double-paint");
}

#[test]
fn group_opacity_multiplies_through_children() {
    // Group opacity 0.5, child opacity 1.0 → effective coverage drops.
    let mut parent = SceneObject {
        id: ObjectId::new(100),
        kind: ObjectKind::Group(vec![ObjectId::new(1)]),
        ..SceneObject::default()
    };
    parent.opacity = 0.5;
    let child = top_left_rect(1, 0.0, 0.0, 16.0, 16.0, 0xFFFFFFFF, 0);

    let mut scene = Scene {
        canvas: Canvas::raster(16, 16),
        background: Background::Solid(0x000000FF),
        ..Scene::default()
    };
    scene.objects.push(parent);
    scene.objects.push(child);

    let mut r = RasterRenderer::new();
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    let px = pixel(&frame.planes[0].data, 16, 8, 8);
    // White at 0.5 over black → mid-grey.
    assert!((90..=165).contains(&px[0]), "expected mid-grey, got {px:?}");
}

#[test]
fn group_with_missing_child_id_does_not_panic() {
    let parent = SceneObject {
        id: ObjectId::new(100),
        kind: ObjectKind::Group(vec![ObjectId::new(42), ObjectId::new(1)]),
        ..SceneObject::default()
    };
    let child = top_left_rect(1, 0.0, 0.0, 8.0, 8.0, 0xFFFFFFFF, 0);

    let mut scene = Scene {
        canvas: Canvas::raster(16, 16),
        background: Background::Transparent,
        ..Scene::default()
    };
    scene.objects.push(parent);
    scene.objects.push(child);

    let mut r = RasterRenderer::new();
    // Should render without erroring; the missing id is silently dropped.
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    assert_eq!(pixel(&frame.planes[0].data, 16, 4, 4)[3], 0xFF);
}

#[test]
fn group_cycle_terminates() {
    // Two groups referencing each other — should terminate at the
    // first repeated visit, not loop forever.
    let g_a = SceneObject {
        id: ObjectId::new(100),
        kind: ObjectKind::Group(vec![ObjectId::new(101)]),
        ..SceneObject::default()
    };
    let g_b = SceneObject {
        id: ObjectId::new(101),
        kind: ObjectKind::Group(vec![ObjectId::new(100)]),
        ..SceneObject::default()
    };
    let mut scene = Scene {
        canvas: Canvas::raster(8, 8),
        background: Background::Transparent,
        ..Scene::default()
    };
    scene.objects.push(g_a);
    scene.objects.push(g_b);

    let mut r = RasterRenderer::new();
    // Smoke: render returns, no panic / stack-overflow.
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    assert_eq!(frame.planes[0].data.len(), 8 * 8 * 4);
}

#[test]
fn shape_path_svg_renders_as_filled_polygon() {
    // Triangle described as SVG path data — pen at top-centre, two
    // diagonals down to the bottom corners, close.
    let path_obj = SceneObject {
        id: ObjectId::new(1),
        kind: ObjectKind::Shape(Shape::Path {
            data: "M16,4 L4,28 L28,28 Z".to_string(),
            fill: 0xFFFFFFFF,
            stroke: None,
        }),
        transform: Transform {
            position: (0.0, 0.0),
            anchor: (0.0, 0.0),
            ..Transform::identity()
        },
        ..SceneObject::default()
    };
    let mut scene = Scene {
        canvas: Canvas::raster(32, 32),
        background: Background::Solid(0x000000FF),
        ..Scene::default()
    };
    scene.objects.push(path_obj);

    let mut r = RasterRenderer::new();
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    let data = &frame.planes[0].data;
    // Inside the triangle (near centroid at ~(16,20)): white.
    let inside = pixel(data, 32, 16, 22);
    assert!(
        inside[0] > 200 && inside[1] > 200 && inside[2] > 200,
        "triangle interior should be white: {inside:?}"
    );
    // Outside the triangle (top-left corner): black backdrop only.
    let outside = pixel(data, 32, 1, 1);
    assert_eq!(outside, [0x00, 0x00, 0x00, 0xFF]);
}

#[test]
fn shape_path_arc_lowers_to_filled_arc_segment() {
    // Quarter-circle wedge: pen at (0, 16), arc up-and-right to
    // (16, 0) with rx = ry = 16, then close back through the origin.
    // The fill should cover the lower-left wedge of a 16-radius circle
    // centred at (16, 16). The parser turns this into an
    // `PathCommand::ArcTo`, oxideav-raster's `flatten_arc_to_cubics`
    // turns it into cubics, and the rasteriser fills the result.
    let arc_obj = SceneObject {
        id: ObjectId::new(1),
        kind: ObjectKind::Shape(Shape::Path {
            data: "M 0 16 A 16 16 0 0 0 16 0 L 0 0 Z".to_string(),
            fill: 0xFFFFFFFF,
            stroke: None,
        }),
        ..SceneObject::default()
    };
    let mut scene = Scene {
        canvas: Canvas::raster(32, 32),
        background: Background::Solid(0x000000FF),
        ..Scene::default()
    };
    scene.objects.push(arc_obj);

    let mut r = RasterRenderer::new();
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    let data = &frame.planes[0].data;
    // Inside the wedge (well clear of the curve): white.
    let inside = pixel(data, 32, 2, 2);
    assert!(
        inside[0] > 200 && inside[1] > 200 && inside[2] > 200,
        "wedge interior should be white: {inside:?}"
    );
    // Far outside the wedge (top-right): the black backdrop is intact.
    let outside = pixel(data, 32, 30, 30);
    assert_eq!(outside, [0x00, 0x00, 0x00, 0xFF]);
}

#[test]
fn shape_path_with_invalid_arc_flag_is_skipped() {
    // A malformed arc flag (`2` instead of `0`/`1`) makes the parser
    // bail; the renderer drops the whole shape rather than partially
    // emitting it. Matches the existing "unparseable → skip silently"
    // contract for any other bad path data.
    let bad = SceneObject {
        id: ObjectId::new(1),
        kind: ObjectKind::Shape(Shape::Path {
            data: "M 0 0 A 5 5 0 2 0 10 10".to_string(),
            fill: 0xFF0000FF,
            stroke: None,
        }),
        ..SceneObject::default()
    };
    let mut scene = Scene {
        canvas: Canvas::raster(16, 16),
        background: Background::Transparent,
        ..Scene::default()
    };
    scene.objects.push(bad);

    let mut r = RasterRenderer::new();
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    let lit = frame.planes[0]
        .data
        .chunks_exact(4)
        .filter(|p| p[3] != 0)
        .count();
    assert_eq!(lit, 0);
}

#[test]
fn group_clip_intersects_with_child_clip_region() {
    // Group with a clip rect that excludes most of the canvas.
    let parent = SceneObject {
        id: ObjectId::new(100),
        kind: ObjectKind::Group(vec![ObjectId::new(1)]),
        clip: Some(ClipRect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        }),
        ..SceneObject::default()
    };
    let child = top_left_rect(1, 0.0, 0.0, 16.0, 16.0, 0xFFFFFFFF, 0);
    let mut scene = Scene {
        canvas: Canvas::raster(16, 16),
        background: Background::Transparent,
        ..Scene::default()
    };
    scene.objects.push(parent);
    scene.objects.push(child);

    let mut r = RasterRenderer::new();
    let frame = r.render_at(&scene, 0).unwrap().video.unwrap();
    let data = &frame.planes[0].data;
    // Inside group clip (4,4): lit.
    assert_eq!(pixel(data, 16, 4, 4)[3], 0xFF);
    // Outside group clip (12,12): clear.
    assert_eq!(pixel(data, 16, 12, 12)[3], 0);
}

#[test]
fn animated_opacity_changes_rendered_coverage() {
    use oxideav_scene::{AnimatedProperty, Animation, Easing, Keyframe, KeyframeValue, Repeat};

    let mut scene = Scene {
        canvas: Canvas::raster(16, 16),
        background: Background::Solid(0x000000FF),
        ..Scene::default()
    };
    let fade = Animation::new(
        AnimatedProperty::Opacity,
        vec![
            Keyframe {
                time: 0,
                value: KeyframeValue::Scalar(0.0),
                easing: None,
            },
            Keyframe {
                time: 100,
                value: KeyframeValue::Scalar(1.0),
                easing: None,
            },
        ],
        Easing::Linear,
        Repeat::Once,
    );
    let mut obj = top_left_rect(1, 0.0, 0.0, 16.0, 16.0, 0xFFFFFFFF, 0);
    obj.animations = vec![fade];
    scene.objects.push(obj);

    let mut r = RasterRenderer::new();
    // At t=0 opacity is 0 → centre stays black; at t=100 opacity is 1 → white.
    let early = r.render_at(&scene, 0).unwrap().video.unwrap();
    let late = r.render_at(&scene, 100).unwrap().video.unwrap();
    let e = pixel(&early.planes[0].data, 16, 8, 8)[0] as i32;
    let l = pixel(&late.planes[0].data, 16, 8, 8)[0] as i32;
    assert!(
        l > e + 150,
        "opacity ramp should brighten: early={e} late={l}"
    );
}
