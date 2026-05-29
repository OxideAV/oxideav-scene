//! End-to-end tests for [`RasterRenderer`] through the public crate
//! surface — compositing backgrounds + shapes into an RGBA frame, and
//! driving the renderer over a finite scene via `RenderedSource`.

use oxideav_scene::{
    Background, Canvas, ObjectId, ObjectKind, RasterRenderer, Scene, SceneObject, SceneRenderer,
    Shape, Transform,
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
