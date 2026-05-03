//! Smoke tests — make sure the scaffold compiles and the data model
//! composes. Real behaviour lives in unit tests inside each module.

use oxideav_scene::{
    AnimatedProperty, Animation, Canvas, Easing, ExportOp, ImageSource, Keyframe, KeyframeValue,
    LengthUnit, Lifetime, ObjectId, ObjectKind, Operation, Page, Repeat, Scene, SceneDuration,
    SceneObject, SceneRenderer, StubRenderer, Transform,
};

#[test]
fn build_a_pdf_page_scene() {
    let mut scene = Scene {
        canvas: Canvas::Vector {
            width: 595.0,
            height: 842.0,
            unit: LengthUnit::Point,
        },
        duration: SceneDuration::Finite(1),
        ..Scene::default()
    };
    scene.objects.push(SceneObject {
        id: ObjectId::new(1),
        kind: ObjectKind::Text(oxideav_scene::TextRun {
            text: "Hello, world".to_string(),
            font_family: "Helvetica".to_string(),
            font_weight: 400,
            font_size: 12.0,
            color: 0x000000FF,
            ..Default::default()
        }),
        ..SceneObject::default()
    });
    assert!(scene.canvas.raster_size().is_none());
    assert_eq!(scene.visible_at(0).len(), 1);
}

#[test]
fn build_a_streaming_compositor_scene() {
    let scene = Scene {
        canvas: Canvas::raster(1280, 720),
        duration: SceneDuration::Indefinite,
        ..Scene::default()
    };
    assert_eq!(scene.canvas.raster_size(), Some((1280, 720)));
    assert!(scene.duration.contains(i64::MAX));
}

#[test]
fn build_an_nle_timeline_scene() {
    let mut scene = Scene {
        canvas: Canvas::raster(1920, 1080),
        duration: SceneDuration::Finite(10_000),
        ..Scene::default()
    };
    // V1: base clip at z=0
    scene.objects.push(SceneObject {
        id: ObjectId::new(1),
        kind: ObjectKind::Image(ImageSource::Path("clip1.png".into())),
        lifetime: Lifetime {
            start: 0,
            end: Some(5_000),
        },
        z_order: 0,
        ..SceneObject::default()
    });
    // V2: overlay with a fade-in on opacity
    let fade_in = Animation::new(
        AnimatedProperty::Opacity,
        vec![
            Keyframe {
                time: 4_000,
                value: KeyframeValue::Scalar(0.0),
                easing: None,
            },
            Keyframe {
                time: 5_000,
                value: KeyframeValue::Scalar(1.0),
                easing: None,
            },
        ],
        Easing::EaseInOut,
        Repeat::Once,
    );
    scene.objects.push(SceneObject {
        id: ObjectId::new(2),
        kind: ObjectKind::Image(ImageSource::Path("overlay.png".into())),
        lifetime: Lifetime {
            start: 4_000,
            end: Some(10_000),
        },
        animations: vec![fade_in],
        z_order: 10,
        ..SceneObject::default()
    });
    scene.sort_by_z_order();
    let visible_at_4500 = scene.visible_at(4_500);
    assert_eq!(visible_at_4500.len(), 2);
    assert_eq!(visible_at_4500[0].z_order, 0);
    assert_eq!(visible_at_4500[1].z_order, 10);
}

#[test]
fn transform_identity_is_neutral() {
    let t = Transform::identity();
    assert_eq!(t.scale, (1.0, 1.0));
    assert_eq!(t.rotation, 0.0);
}

#[test]
fn operation_enum_is_non_exhaustive_friendly() {
    // Constructs each variant to make sure the public shape is
    // usable from downstream code.
    let _ = Operation::RemoveObject {
        id: ObjectId::new(1),
        at: 0,
    };
    let _ = Operation::EndScene;
    let _ = ExportOp::Raw {
        format: "pdf",
        payload: Vec::new(),
    };
}

#[test]
fn stub_renderer_surfaces_unsupported() {
    let mut r = StubRenderer;
    let scene = Scene::default();
    assert!(r.prepare(&scene).is_err());
}

#[test]
fn build_a_scene_with_vector_object() {
    use oxideav_core::{Group, TimeBase, VectorFrame};
    let vf = VectorFrame {
        width: 100.0,
        height: 100.0,
        view_box: None,
        root: Group::default(),
        pts: None,
        time_base: TimeBase::new(1, 1),
    };
    let mut scene = Scene::default();
    scene.objects.push(SceneObject {
        id: ObjectId::new(42),
        kind: ObjectKind::Vector(vf),
        ..SceneObject::default()
    });
    assert_eq!(scene.objects.len(), 1);
    assert!(matches!(scene.objects[0].kind, ObjectKind::Vector(_)));
}

#[cfg(feature = "raster")]
#[test]
fn rasterize_vector_object_smoke() {
    use oxideav_core::{Group, TimeBase, VectorFrame};
    let vf = VectorFrame {
        width: 32.0,
        height: 16.0,
        view_box: None,
        root: Group::default(),
        pts: None,
        time_base: TimeBase::new(1, 1),
    };
    let frame = oxideav_scene::rasterize_vector(&vf, 32, 16);
    assert!(!frame.planes.is_empty());
    assert_eq!(frame.planes[0].data.len(), 32 * 16 * 4);
}

#[test]
fn build_a_paged_pdf_scene() {
    // A4 cover, then two US Letter body pages — varied page sizes
    // are the whole point of the pages-mode model.
    let scene = Scene {
        canvas: Canvas::Vector {
            width: 595.0,
            height: 842.0,
            unit: LengthUnit::Point,
        },
        pages: Some(vec![
            Page::new(595.0, 842.0),
            Page::new(612.0, 792.0),
            Page::new(612.0, 792.0),
        ]),
        ..Scene::default()
    };
    assert!(scene.is_paged());
    assert_eq!(scene.pages.as_ref().unwrap().len(), 3);
    let tl = scene.pages_to_timeline(2_000);
    assert_eq!(tl, vec![(0, 0), (1, 2_000), (2, 4_000)]);
}
