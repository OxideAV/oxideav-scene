# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `Metadata`: `creator` (authoring tool, distinct from `producer` =
  output writer — mirrors PDF `/Info`'s `/Creator` vs `/Producer`),
  `modified_at` (ISO-8601, mirrors `created_at`), and
  `custom: BTreeMap<String, String>` for per-format extras (PDF
  `/Info` custom keys, Matroska `ContentTrack` tags, RDF properties,
  mp4 `udta` items, etc).
- `page::Page` — a single page in a paged-content scene. Carries
  per-page `width / height`, an `oxideav_core::VectorFrame` payload,
  an optional human-readable `label` (PDF `/Info` page labels), and
  a `0/90/180/270` `orientation`.
- `Scene::pages: Option<Vec<Page>>` — additive sibling of `duration`.
  `Some(...)` puts the scene into pages mode (PDF / multi-page TIFF
  writers); `None` keeps it in timeline mode (PNG / MP4 / RTMP
  writers, the existing default).
- `Scene::is_paged`, `Scene::pages_to_timeline`,
  `Scene::timeline_to_pages` — adapters between the two modes.
- `SourceFormat::paged` — `true` when the source scene is in pages
  mode, so paged-content sinks can reject timeline scenes (and vice
  versa) early in `init()`.

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
