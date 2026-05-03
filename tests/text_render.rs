//! Integration test: a `TextRun` parked on a `SceneObject` is
//! rasterised end-to-end through the public [`oxideav_scene::TextRenderer`]
//! surface. Replaces the round-1 "scaffold" expectation that scene
//! couldn't actually render text.
//!
//! Skips silently if `crates/oxideav-ttf/tests/fixtures/DejaVuSans.ttf`
//! isn't reachable from this crate's working directory — the same
//! pattern `oxideav_subtitle::compositor::tests::scribe_path_renders_text`
//! uses.

use oxideav_scene::{ObjectId, ObjectKind, SceneObject, TextRenderer, TextRun};

fn load_dejavu() -> Option<oxideav_scribe::Face> {
    let candidates = [
        "../oxideav-ttf/tests/fixtures/DejaVuSans.ttf",
        "tests/fixtures/DejaVuSans.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            return oxideav_scribe::Face::from_ttf_bytes(bytes).ok();
        }
    }
    None
}

#[test]
fn scene_text_object_rasterises_end_to_end() {
    let face = match load_dejavu() {
        Some(f) => f,
        None => return,
    };

    // Build a SceneObject carrying a TextRun. This is the same shape
    // a downstream caller would put into Scene::objects.
    let text_run = TextRun {
        text: "Hello, world!".to_string(),
        font_family: "DejaVu Sans".to_string(),
        font_weight: 400,
        font_size: 24.0,
        color: 0xFFFFFFFF,
        advances: None,
        italic: false,
        underline: false,
    };
    let obj = SceneObject {
        id: ObjectId::new(1),
        kind: ObjectKind::Text(text_run.clone()),
        ..SceneObject::default()
    };
    // Sanity: the object actually carries a Text payload.
    assert!(matches!(obj.kind, ObjectKind::Text(_)));

    // Pull the run back out and feed it to the renderer.
    let run = match &obj.kind {
        ObjectKind::Text(r) => r,
        _ => panic!("kind mismatch"),
    };

    let dst_w: u32 = 200;
    let dst_h: u32 = 40;
    let mut dst = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];

    let mut tr = TextRenderer::new(face);
    tr.render_run_into(run, &mut dst, dst_w, dst_h, 5, 5)
        .unwrap();

    // Total lit pixel count is non-trivial.
    let lit = dst.chunks_exact(4).filter(|p| p[3] > 0).count();
    assert!(
        lit > 100,
        "scene TextRenderer produced only {lit} lit pixels — expected real glyph coverage"
    );
}

#[test]
fn scene_wrapped_text_stacks_lines() {
    let face = match load_dejavu() {
        Some(f) => f,
        None => return,
    };
    let mut tr = TextRenderer::new(face);
    let run = TextRun {
        text: "one two three four five six seven eight nine ten".to_string(),
        font_family: "DejaVu Sans".to_string(),
        font_weight: 400,
        font_size: 16.0,
        color: 0xFFFFFFFF,
        advances: None,
        italic: false,
        underline: false,
    };
    let lines = tr.render_run_wrapped(&run, 100.0).unwrap();
    assert!(
        lines.len() >= 2,
        "expected wrapping at 100 px to yield multiple lines; got {}",
        lines.len()
    );

    let dst_w: u32 = 120;
    let dst_h: u32 = 200;
    let mut dst = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    tr.render_run_wrapped_into(&run, &mut dst, dst_w, dst_h, 4, 4, 100.0, None)
        .unwrap();

    // Lit pixels must show up at multiple distinct y-rows. Counting
    // unique non-empty rows gives a stronger "stacking happened"
    // signal than splitting on a fixed-height boundary (which can
    // miss when several short lines pack into the upper half).
    let mut nonempty_rows = std::collections::BTreeSet::new();
    for y in 0..dst_h {
        for x in 0..dst_w {
            let off = (y as usize * dst_w as usize + x as usize) * 4;
            if dst[off + 3] > 0 {
                nonempty_rows.insert(y);
                break;
            }
        }
    }
    assert!(
        !nonempty_rows.is_empty(),
        "no lit pixels at all — wrapping path produced an empty canvas"
    );
    // For text wrapped at 100 px with a 16 px font we expect at
    // least two visually-separated bands of rows (line 1 + line 2).
    // A single line is at most ~ascent+descent rows tall (≈ 22 px
    // for DejaVu @ 16 px); seeing >25 distinct rows means line 2
    // landed below line 1 with a gap.
    assert!(
        nonempty_rows.len() > 25,
        "only {} distinct rows have lit pixels; expected wrapping to stack ≥ 2 lines",
        nonempty_rows.len()
    );
}
