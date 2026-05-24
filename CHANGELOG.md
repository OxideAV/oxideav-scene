# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
