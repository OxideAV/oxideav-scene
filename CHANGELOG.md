# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `Background::DecodedImage(Arc<VideoFrame>)` — new background variant
  symmetric with `ImageSource::Decoded` on the object side, carrying a
  pre-decoded straight-alpha RGBA8 frame for use as a full-canvas
  backdrop without invoking a decoder. `RasterRenderer::build_frame`
  lowers it into an `oxideav_core::Node::Image` wrapped in an
  `ImageRef` whose `bounds` rectangle spans the full canvas
  `(0, 0)..(w, h)`, so the downstream `oxideav_raster::Renderer`
  stretches the source frame across the backdrop via its configured
  `ImageFilter` (bilinear by default). The carried `VideoFrame` is
  read under the canonical RGBA8-stride convention shared with the
  object-side `Decoded` arm (`width = stride / 4`,
  `height = data.len() / stride`); frames whose first plane doesn't
  satisfy that convention skip silently (the backdrop reduces to
  `Background::Transparent`). The path-based `Background::Image(_)`
  variant continues to skip — pre-decode upstream and feed back via
  this variant until a decoder-aware renderer lands. 5 new tests
  cover backdrop node emission, full-canvas pixel coverage of a
  constant-colour source, painter's-order composition with foreground
  objects, the degenerate-stride drop, and the path-variant skip path.
- `RasterRenderer` now lowers `ObjectKind::Image(ImageSource::Decoded)`
  into an `oxideav_core::Node::Image` and composites it through
  `oxideav_raster::Renderer::draw_image` — pre-decoded straight-alpha
  RGBA8 bitmaps participate in the same paint-order walk as backgrounds,
  shapes, vector frames, and groups, under each object's animation-
  merged `Transform` / opacity / clip. The renderer reads the carried
  `VideoFrame`'s natural pixel dimensions from the canonical RGBA8-stride
  convention (`width = stride / 4`, `height = data.len() / stride`),
  matching the convention the raster crate itself emits and reads at
  the `Node::Image` sampling boundary, so a frame produced by
  `oxideav_raster::Renderer::render` round-trips through `Decoded(_)`
  without an intermediate conversion. Encoded variants
  (`ImageSource::Path` / `ImageSource::EncodedBytes`) continue to skip
  silently — pre-decode upstream and feed back via `Decoded(_)` for now.
- `ImageSource::natural_size()` exposes the same RGBA8-stride decoding
  on `ImageSource::Decoded`; `ObjectKind::Image(_).content_size()` now
  reports `Some((w, h))` for decoded image sources (and propagates
  through `SceneObject::content_size` / `bbox`). Encoded variants still
  return `None`.
- `svg_path` now parses elliptical arc commands `A / a` per SVG 1.1
  F.6.1 — the grammar's special-cased single-digit `fA` / `fS` flag
  tokens (which may abut the following number, e.g. `A5,5 0 0010,10`
  → `rx=5 ry=5 rot=0 fA=0 fS=0 x=10 y=10`) parse via a dedicated
  `read_flag` helper. Arcs lower into
  `oxideav_core::PathCommand::ArcTo`: `x_axis_rot` is converted from
  SVG degrees to radians, flags map to the `large_arc` / `sweep`
  booleans, and the F.6.2 out-of-range rules apply at parse time —
  negative radii are taken absolute, `rx = 0` or `ry = 0` becomes a
  straight `line_to`, coincident endpoints are silently omitted. The
  downstream raster pipeline already flattens `PathCommand::ArcTo`
  via `oxideav_raster::flatten_arc_to_cubics`, so path data with
  arcs now renders end-to-end through `RasterRenderer` rather than
  being dropped. Bad flag tokens raise the new
  `SvgPathError::InvalidArcFlag`.
- `svg_path` module — minimal SVG 1.1 path-data parser
  (`parse_path` → `oxideav_core::Path`, plus `parse_bbox` for an
  AABB summary). Supports the full SVG 1.1 path-data command set:
  `M / m`, `L / l`, `H / h`, `V / v`, `C / c`, `S / s`, `Q / q`,
  `T / t`, `A / a`, `Z / z`. Number lexer accepts integers, signed
  decimals, leading- / trailing-dot decimals, and scientific
  notation. Re-exported at the crate root as `parse_svg_path` +
  `SvgPathError`. 28 unit tests cover commands, separators,
  smooth-curve reflection, arc-grammar (absolute / relative /
  minified flag-abutting / zero-radius / negative-radius /
  coincident-endpoint / chained arc-tuples), and the
  truncated-input / invalid-flag error paths.
- `parse_bbox` now extends its conservative AABB across arc
  segments — each `PathCommand::ArcTo` expands both endpoints by
  `max(|rx|, |ry|)` on each axis, which is a rotation-agnostic
  strict superset of the true elliptic-arc bound (any point on the
  arc lies within `max(rx, ry)` of *both* endpoints). Matches the
  convex-hull-of-control-points style already used for cubics and
  quads. `Shape::Path` content size now reports a usable bound for
  arc-using paths instead of `None`.
- `RasterRenderer` now lowers SVG arc commands through to filled
  geometry: the parser hands `PathCommand::ArcTo` to
  `oxideav_raster::flatten_arc_to_cubics` inside the path-flattener,
  so a wedge described as `M 0 16 A 16 16 0 0 0 16 0 L 0 0 Z`
  rasterises as the expected quarter-disc fill. Covered by a new
  `shape_path_arc_lowers_to_filled_arc_segment` integration test;
  the previous `shape_path_with_arc_skips_without_error` test is
  replaced by `shape_path_with_invalid_arc_flag_is_skipped` which
  exercises the "parser bail → renderer drops the shape" path.
- `RasterRenderer` now lowers `Shape::Path` through `svg_path` —
  parseable SVG paths render as filled (+ optionally stroked)
  geometry; unparseable data (including arc commands) is skipped
  without erroring the frame.
- `Shape::content_size` reports the AABB of every anchor / control
  point for `Shape::Path` (via `svg_path::parse_bbox`) instead of
  returning `None`. The bound is the convex-hull-of-control-points
  superset of the painted curve — a tighter bound would need to
  walk the Bezier derivative roots, which scene-layer layout
  queries don't need.
- `RasterRenderer` resolves `ObjectKind::Group` containers — each
  child id is looked up in the scene, the child is sampled at the
  current time (so its own animations are honoured), and the
  lowered child node is wrapped under the parent group's animation-
  merged `Transform` / `opacity` / `clip`. Cycles in the child
  graph are broken at the second visit (a visited-id set is forked
  per child); missing ids are silently dropped; dead children
  (`Lifetime::is_live_at(t) == false`) are excluded. Children
  referenced from any group are claimed by their parent and skipped
  at the top level so they don't paint twice. 5 integration tests
  cover composition, opacity multiplication, missing-id tolerance,
  cycle termination, and group-clip intersection.

## [0.1.4](https://github.com/OxideAV/oxideav-scene/compare/v0.1.3...v0.1.4) - 2026-05-29

### Other

- RasterRenderer — concrete SceneRenderer for the vector slice
- per-frame Sample + animation-track composition helpers
- per-object + scene-wide AABB queries (bbox, hit_test)
- typed matrix lowering + axis-aligned bbox accessors
- typed paint patterns + Scene::apply / merge driver APIs
- drop committed Cargo.lock + relax oxideav-core to "0.1"

### Added

- `RasterRenderer` — a concrete `SceneRenderer` (in
  `src/raster_renderer.rs`, re-exported at the crate root) that walks
  `Scene::sampled_at(t)` in paint order and composites the vector slice
  of a scene into an RGBA8 `oxideav_core::VideoFrame` via
  `oxideav_raster::Renderer`. Handles backgrounds (`Solid`,
  `Transparent`, two-colour `LinearGradient`, multi-stop
  `Background::Gradient` linear + radial), `ObjectKind::Shape`
  (`Rect` with corner radius, `Polygon`), and `ObjectKind::Vector`
  (root group inlined under the object transform). Honours each
  object's animation-merged `Transform` (lowered via
  `Transform::to_matrix`), `opacity` (group alpha), and `clip` rect
  (group clip path). `Image` / `Video` / `Live` / `Text` / `Group`
  kinds and `Shape::Path` (opaque SVG data) are skipped without error
  pending a font-registry / decoder-aware renderer. `Canvas::Vector`
  scenes are rejected with `Error::Unsupported`. `RasterRenderer::seek`
  is a no-op (the renderer rebuilds each frame from scratch).
  `RasterRenderer::build_frame(scene, t)` exposes the intermediate
  `VectorFrame` for callers that want the vector tree without
  rasterising. Covered by 11 unit tests + `tests/raster_render.rs`
  (background + layered shapes, finite-scene drive via
  `RenderedSource`, animated-opacity coverage ramp).
- `Shape::content_size()` — object-local `(width, height)` for the
  shape's filled geometry. `Rect` reports its declared dims; `Polygon`
  reports the AABB of its `points` (empty polygon → `(0, 0)`); `Path`
  is opaque (`None`). Stroke half-widths are excluded.
- `ObjectKind::content_size()` / `SceneObject::content_size()` —
  intrinsic content extent for kinds that carry one. `Vector` pulls
  from the underlying `oxideav_core::VectorFrame` viewport, `Shape`
  delegates to `Shape::content_size`, `Live` uses `hint_size`;
  `Image` / `Video` / `Text` / `Group` return `None`.
- `SceneObject::bbox(fallback)` — axis-aligned bounding box of the
  object in canvas space. Uses the object's intrinsic
  `content_size` when available, otherwise the caller-supplied
  `fallback`; pipes through `Transform::bbox`; intersects with
  `ClipRect` (zero-extent on no overlap so callers can cull).
- `Scene::bbox_at(t, fallback)` — union AABB of every object live
  at scene time `t`. Skips dead objects and zero-extent (clipped-
  out) objects so they don't pull the union toward their corners.
  Geometric footprint only — opacity / blend / effects are not
  considered.
- `Scene::hit_test_at(t, point, fallback)` — top-most live
  object's `ObjectId` whose AABB contains `point`. Painter's-
  algorithm order: higher `z_order` wins, ties broken by later
  insertion in `Scene::objects`. AABB-only (not per-pixel shape
  containment).
- Backing property-test suite at
  `tests/scene_geometry_props.rs`: deterministic xorshift PRNG
  drives 7 invariants (empty scene → `None`, single-object
  identity, member-coverage, dead-object skipping, top-z-order
  hit, miss outside, clip-collapses-contribution).
- `paint` module: `Stop`, `Gradient` (multi-stop linear / radial),
  `Paint` typed paint patterns. `Gradient::sample(t)` evaluates the
  gradient at a normalised axis position via per-channel linear
  interpolation (bit-identical to
  `KeyframeValue::Color`'s lerp). All three types re-exported at
  the crate root.
- `Background::Gradient(Gradient)` — richer alternative to the
  legacy two-colour `Background::LinearGradient { from, to,
  angle_deg }`. Both variants coexist; the new one carries any
  number of stops and supports radial fills.
- `Scene::apply(op)` / `Scene::apply_batch(ops)` — in-process driver
  for the `Operation` enum. Returns short receipts (`"add obj#7"`,
  `"animate obj#3"`, …) suitable for compositor logs. Operations on
  non-existent object ids return `Err("object id not found")`;
  `apply_batch` stops at the first error and returns the receipts
  gathered so far.
- `Scene::merge(other, time_offset, z_offset)` — splices another
  scene onto this one. Shifts object lifetimes + animation keyframe
  times by `time_offset`, offsets `z_order` by `z_offset`, appends
  audio cues with shifted triggers, and extends `Finite` durations
  to cover any reach past the current end.
- `Scene::next_object_id()` — allocates a fresh `ObjectId`
  guaranteed not to collide with any existing object in the scene
  (`max(id) + 1`).
- `Transform::to_matrix(width, height)` — lowers the high-level
  position / scale / rotation / anchor / skew transform into a flat
  `oxideav_core::Transform2D` (the SVG / PDF `matrix(a,b,c,d,e,f)`
  form), realising the documented application order with the
  normalised anchor resolved against the given content size.
- `Transform::apply_to_point(width, height, point)` — maps an
  object-local `oxideav_core::Point` into canvas space; sugar over
  `to_matrix().apply()`.
- `Transform::bbox(width, height)` — axis-aligned `oxideav_core::Rect`
  enclosing a `(width, height)` content box after the transform.
  Tight for translate / scale / skew, rotation-aware (grows to
  contain a rotated rectangle), with non-negative extent. Backed by a
  deterministic property-test suite (`tests/transform_props.rs`):
  identity no-op, helper/matrix agreement, AABB corner-containment +
  tightness, rotation area lower-bound, and translation commutativity.
- `SceneObject::evaluate_property_at(t, prop)` — raw lookup of the
  first matching `Animation` track's `KeyframeValue` at scene time `t`.
  `None` when no track targets `prop` or when the track has no
  keyframes; the second of two same-property tracks is shadowed.
- `SceneObject::effective_transform_at(t)` — base `Transform`
  composed with `Position` / `Scale` / `Rotation` / `Skew` / `Anchor`
  animation tracks evaluated at `t`. Per-property rule: `Position` +
  `Rotation` + `Skew` add to base, `Scale` multiplies, `Anchor`
  replaces. Variant mismatches (e.g. a `Scalar` keyframe on a
  `Position` track) leave the base value alone.
- `SceneObject::effective_opacity_at(t)` — base `opacity` multiplied
  by any `Opacity` animation track's `Scalar` value at `t`, then
  clamped to `0.0..=1.0` so the result is compositor-safe.
- `SceneObject::sample_at(t)` / `Scene::sampled_at(t)` — per-frame
  resolved state. `sample_at` returns the new `Sample` struct
  carrying `(id, z_order, transform, opacity, blend_mode, clip)` —
  the renderer's flat per-object view. `Scene::sampled_at(t)`
  collects every live object's `Sample` in paint order (z ascending,
  ties broken by insertion). `Sample` is re-exported at the crate
  root.


## [0.1.3](https://github.com/OxideAV/oxideav-scene/compare/v0.1.2...v0.1.3) - 2026-05-04

### Other

- migrate TextRenderer to vector pipeline (scribe + raster)

### Changed

- `text::TextRenderer`: migrated from the (now-removed)
  `oxideav-scribe` pixel pipeline (`Composer` / `render_text` /
  `render_text_wrapped` / `RgbaBitmap` re-export, dropped in scribe
  0.1.5 / #354) to the vector path. Internally the renderer now wraps
  the face in a `FaceChain`, calls `Shaper::shape_to_paths` to produce
  positioned glyph nodes, recolours the default-black `PathNode.fill`
  to the run's colour, and rasterises through `oxideav_raster::Renderer`
  with the glyph-cache `Group::cache_key` envelope intact (so the same
  glyph at the same size hits the bitmap cache across runs).
- `RgbaBitmap`: now defined locally in `oxideav_scene::text` (mirrors
  the former `oxideav_scribe::RgbaBitmap` byte-layout — same `width`,
  `height`, packed-RGBA8 `data`). The `TextRenderer` public API
  (`render_run`, `render_run_into`, `render_run_wrapped`,
  `render_run_wrapped_into`, `compose_run_at`) is byte-stable.
- `oxideav-raster` is now a hard dependency (was previously gated
  behind the `raster` cargo feature). The `raster` feature is
  preserved as a no-op for back-compat. `TextRenderer` requires the
  vector→pixel pipeline to function, so vector-only consumers can no
  longer drop the rasteriser dep by disabling the feature; that's a
  follow-up for a future round if anyone needs it.

## [0.1.2](https://github.com/OxideAV/oxideav-scene/compare/v0.1.1...v0.1.2) - 2026-05-03

### Fixed

- *(clippy)* use is_some_and over map_or with false default

### Other

- ObjectKind::Vector + raster fallback ([#350](https://github.com/OxideAV/oxideav-scene/pull/350))
- pages-mode timing model (Page + Scene::pages)
- extend Metadata with creator/modified_at/custom
- bump oxideav-scribe pin to 0.1
- replace never-match regex with semver_check = false
- cargo fmt: fix rustfmt --check CI gate
- drop nested [workspace] + [patch.crates-io] (umbrella sweep)
- real Scribe-backed TextRun renderer (replaces scaffold)
- migrate to centralized OxideAV/.github reusable workflows
- adopt slim VideoFrame shape
- pin release-plz to patch-only bumps

## [0.1.1](https://github.com/OxideAV/oxideav-scene/compare/v0.1.0...v0.1.1) - 2026-04-25

### Other

- release v0.0.2

## [0.1.0](https://github.com/OxideAV/oxideav-scene/compare/v0.0.1...v0.1.0) - 2026-04-25

### Other

- promote to 0.1.0 as required for internal elements
- clarify SceneObject style + axis↔source validity matrix
- revise scene-unified per user feedback (no scene nesting)
- unified scene system proposal
- auto-adapt pixel format between sources and sinks
- add framerate + Source/Sink traits
