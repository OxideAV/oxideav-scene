# oxideav-scene

A **time-based composition model** for oxideav: a `Scene` is a canvas
populated with `Object`s (images, videos, text, shapes, audio cues)
animated over a timeline. Scenes are the foundation for three distinct
workloads:

1. **Document layout** — a PDF page is a single-frame scene with text,
   vector shapes, and image objects laid out in their native
   coordinate system. Edits (adding a watermark, moving an image,
   rewrapping a paragraph) happen on the scene, not on rasterised
   pixels, so text stays selectable and vectors stay crisp on
   re-export.
2. **Live streaming compositor** — a long-running scene fed by external
   operations (`AddObject`, `MoveObject`, `FadeOut`). Intended to sit
   behind an RTMP server so a remote control plane can drive a
   per-viewer overlay: add a lower-third during a goal, slide a logo
   in, trigger a sound effect.
3. **Non-linear video editor (NLE) timeline** — Premiere/Resolve-style
   multi-track editing. Tracks are ordered groups of scene objects,
   transitions are keyframed cross-fades / wipes, effects are filter
   chains attached to a single object.

Zero C dependencies — pure Rust, same rules as the rest of oxideav.

## Status

**Type model complete; vector renderer landed.** This crate ships the
type model + public-API shape for all three use cases, plus two concrete
renderers. Encoding and file-format I/O are still follow-ups.

- `Scene`, `SceneObject`, `ObjectKind`, `Transform`, `Animation`,
  `Keyframe`, `Easing`, `AudioCue` types are in place.
- `RasterRenderer` is a concrete `SceneRenderer`: it walks
  `Scene::sampled_at(t)` in paint order and composites the **vector
  slice** of a scene — backgrounds (solid / transparent / linear +
  radial gradient, plus **`Background::DecodedImage(Arc<VideoFrame>)`**
  — a pre-decoded straight-alpha RGBA8 backdrop wrapped in a
  `Node::Image` spanning the full canvas, stretched edge-to-edge by
  the downstream raster sampler), `Shape` objects (rect with corner
  radius, polygon, **SVG-`path`-data**), `ObjectKind::Vector` frames,
  **`ObjectKind::Group`** containers (children resolved by id and
  inlined under the group's transform / opacity / clip; cycles
  terminated; missing ids dropped), and
  **`ObjectKind::Image(ImageSource::Decoded)`** — the carried
  `oxideav_core::VideoFrame` is wrapped in a `Node::Image` whose
  bounds rectangle spans the frame's natural `(width, height)`
  decoded under the canonical RGBA8-stride convention
  (`width = stride / 4`, `height = data.len() / stride`), so a
  pre-decoded bitmap composites in the same pass as the rest of
  the vector slice under the object's animation-merged transform /
  opacity / clip. The downstream `oxideav_raster::Renderer`
  samples through its configured `ImageFilter` (bilinear by
  default). The result is an RGBA8 `VideoFrame`. `ObjectKind::Video`
  now lowers the new `VideoSource::DecodedFrames { frames,
  frame_duration }` variant symmetrically — at scene time `t` the
  renderer picks
  `frames[((t - lifetime.start) / frame_duration).clamp(0, len-1)]`
  (so a finished clip freezes on its final frame instead of
  flashing black) and wraps the chosen frame in the same
  `Node::Image` shape `Image(Decoded)` uses, so a fixed-resolution
  sequence composites under each object's animation-merged
  `Transform` / opacity / clip in the same paint pass as
  backgrounds, shapes, vector frames, images, and groups. A
  `frame_duration <= 0` falls back to frame 0 instead of dividing
  by zero. Decoder-bound `ImageSource::Path` /
  `ImageSource::EncodedBytes`, `VideoSource::Path` /
  `VideoSource::EncodedBytes`, and the path-based
  `Background::Image(_)` continue to skip silently — pre-decode
  upstream and feed back via the respective `Decoded(_)` /
  `DecodedFrames { .. }` / `DecodedImage(_)` variants for now.
  `ObjectKind::Live` / `ObjectKind::Text` are skipped pending a
  font-registry / live-source-aware renderer. `Canvas::Vector`
  scenes are rejected with `Error::Unsupported` (they export their
  `VectorFrame` directly without rasterisation).
- `ImageSource::natural_size()` (and via it
  `ObjectKind::Image(_).content_size()`) report the carried frame's
  natural pixel dimensions for `ImageSource::Decoded` under the same
  RGBA8-stride convention the renderer reads. Encoded variants still
  return `None` — extracting their natural dimensions would require
  a decoder the scene crate doesn't bind.
- `VideoSource::natural_size()` / `VideoSource::frame_at(t,
  lifetime_start)` (and via the former
  `ObjectKind::Video(_).content_size()`) decode the carried first
  frame's pixel dimensions for `VideoSource::DecodedFrames` under the
  same RGBA8-stride convention, and resolve the visible frame at a
  given scene time inside the carrying object's lifetime. Decoder-
  bound variants return `None` for both, mirroring the `ImageSource`
  shape.
- `svg_path::parse_path` / `parse_svg_path` (re-exported at the crate
  root) lowers an SVG 1.1 path-data string into an
  `oxideav_core::Path`. The supported commands cover the entire SVG
  1.1 path-data grammar: `M / m`, `L / l`, `H / h`, `V / v`,
  `C / c`, `S / s`, `Q / q`, `T / t`, `A / a`, `Z / z`. Elliptical
  arcs lower into `PathCommand::ArcTo` — `x_axis_rot` is normalised
  from SVG degrees to radians, flags map to `large_arc` / `sweep`
  booleans, and the SVG 1.1 F.6.2 out-of-range rules (negative radii
  taken absolute, zero radius → line-to, coincident endpoints →
  omitted segment) are applied at parse time. The downstream
  `oxideav-raster` pipeline flattens the arc IR variant into cubics
  via `flatten_arc_to_cubics`, so path data round-trips parser →
  pixels without a scene-layer flattening pass. The parser feeds
  `Shape::Path` rendering and `Shape::content_size` bbox queries.
- `RasterRenderer::render_at(scene, t)` now also mixes the scene's
  `AudioCue`s into the `RenderedFrame::audio` slot. The renderer
  tracks an internal `audio_cursor` (next-sample scene tick); each
  `render_at(scene, t)` emits a mono `Vec<f32>` covering
  `[audio_cursor, t)` at `scene.sample_rate`, then advances the
  cursor to `t`. `prepare(scene)` resets the cursor to `0`; `seek(t)`
  snaps it to `t`; a rewind render (without a prior `seek`) returns
  an empty audio slice and leaves the cursor where it was. Supported
  sources: `Generator::Silence` / `Generator::SineWave` /
  `Generator::WhiteNoise` (xorshift seeded from the scene-sample
  index since trigger, so the noise is chunk-independent), `PcmS16`
  (`/ 32768.0`), and `PcmF32`. Stereo / multichannel PCM downmixes
  by averaging across channels; source sample rates that differ from
  the scene's resample by nearest-neighbour. The summed mix is
  multiplied by each cue's `volume` `Animation` (empty-keyframes-list
  → unity gain) and clipped to `[-1.0, 1.0]`. The free function
  `mix_cues(scene, start, end)` exposes the same mixer for callers
  that want the audio path without invoking the visual renderer.
  Decoder-bound `AudioSource::Path` / `AudioSource::EncodedBytes`
  continue to skip silently — pre-decode upstream and feed back via
  a PCM variant for now.
- `SceneRenderer` + `SceneSampler` traits are defined; `StubRenderer`
  remains as the always-`Error::Unsupported` placeholder.
- `Paint` + `Gradient` typed paint patterns (multi-stop linear /
  radial) land in [`paint`], with `Background::Gradient(_)` exposing
  them as a richer alternative to the legacy two-colour
  `Background::LinearGradient { from, to, angle_deg }`.
- `Scene::apply(op)` / `Scene::apply_batch(ops)` drive the
  [`Operation`] DSL in-process: add/remove objects, set transforms,
  animate / cancel, fire audio cues. Receipts go to the caller for
  logging.
- `Scene::merge(other, time_offset, z_offset)` splices an entire
  other scene onto this one — appends + shifts lifetimes and
  keyframe times, offsets z-order, extends `SceneDuration::Finite` if
  needed. NLE-style "compose track then append" lands cleanly.
- `Scene::next_object_id()` allocates a collision-free
  monotonically-increasing object id; pair with
  `Operation::AddObject` to keep the streaming-compositor wire
  format short.
- `Light` / `LightCommon` / `SpotParams` typed punctual-light
  primitive in the `light` module — a first 3D-adjacent surface,
  parameterised per the glTF 2.0 ratified extension for punctual
  lights. Three variants — `Directional` / `Point` / `Spot` — share
  `name` / linear-RGB `color` / `intensity` / optional `range`
  distance cutoff; `Spot` adds `inner_cone_angle` / `outer_cone_angle`
  (radians, defaults `0.0` / `PI/4`). Helpers: `is_directional` /
  `is_point` / `is_spot`, `has_position`, `has_direction`,
  `honours_range`, `spot_params()`, and
  `distance_attenuation(distance)` implementing the recommended
  `max(min(1 − (d/range)^4, 1), 0) / d²` rule (falls back to `1/d²`
  with no range, returns `1.0` for the directional variant, clamps
  NaN / non-positive distances to `1.0`). `SpotParams::is_valid`
  enforces `0 ≤ inner < outer ≤ π/2`. Renderer-side integration is a
  follow-up — the type is exposed so 3D-scene importers have a typed
  landing place.
- **`LightInstance` + `Scene::lights`** — typed pose-carrying wrapper
  around `Light` plus a top-level list on `Scene`, so 3D-scene
  importers / writers can round-trip a scene's lights without a full
  3D node graph. `LightInstance` carries `light: Light`,
  `position: [f32; 3]`, and `direction: [f32; 3]` (the world-space
  emission direction; default `[0, 0, -1]` matches the untransformed
  local emission axis the punctual-light contract documents).
  Builders: `LightInstance::new(light)` constructs at the origin
  emitting along `-z`; `with_position` / `with_direction` override
  either pose component. `position_is_meaningful()` /
  `direction_is_meaningful()` route through `Light::has_position` /
  `has_direction` so callers can branch by variant;
  `normalized_direction()` returns the unit-length direction (or
  `None` when the stored vector is degenerate or the variant ignores
  direction — `Point` lights are omnidirectional, so any stored
  direction reads as `None`). `vector_to(world_point)` returns
  `(distance, unit_direction)` from the light position to a world
  point — `None` for the directional variant (the light is at
  infinity) and for coincident / non-finite geometry, so renderers
  don't have to special-case the div-by-zero / NaN paths.
  `cone_attenuation(world_point)` returns the spot cone's angular
  falloff per the punctual-light cosine-interpolation formula
  (`scale = 1 / max(1e-3, cos(inner) - cos(outer))`,
  `angular = saturate(cd * scale + offset)`, squared), returning
  `Some(1.0)` for directional + point lights so consumers can fold
  it into a `(distance × cone)` product uniformly across variants.
  `irradiance_at(world_point)` folds the whole composition into one
  per-channel linear-RGB triple a renderer multiplies against a
  surface's reflectance:
  `L_c = color[c] × intensity × distance_attenuation × cone_attenuation`.
  Directional lights return `Some(color × intensity)` at every point
  (un-attenuated parallel rays); point / spot lights scale that base
  by the inverse-square distance window (a point beyond `range` yields
  the zero triple) and, for spots, the cone falloff. Returns `None`
  for geometry too degenerate to shade (non-finite query, or a point
  coincident with a positional light), and is deliberately unclamped
  (physical inverse-square × intensity can exceed unity — tone-mapping
  is the consumer's job).
  `Scene::lights: Vec<LightInstance>` is default-empty; helpers
  `push_light` / `has_lights` / `lights_filter(predicate)` cover
  the common access patterns.
  `Scene::merge` concatenates the other scene's lights verbatim
  (no timeline component yet). The 2D `RasterRenderer` ignores this
  list — light contribution to raster composition is follow-up
  work; for now the field is the typed landing place for
  glTF / USD / OBJ readers.
- **`Material` + `Scene::materials`** — typed PBR material surface in
  the `material` module, the companion to the punctual lights: where
  a light describes the energy arriving at a surface
  (`LightInstance::irradiance_at`), a `Material` describes how the
  surface responds. Metallic-roughness model per the glTF 2.0 core
  specification, defaults tracking the spec exactly:
  `PbrMetallicRoughness` carries the linear-RGBA `base_color_factor`
  (default white), `metallic_factor` / `roughness_factor` (default
  `1.0` each), and optional base-color / packed metallic-roughness
  texture slots; `Material` wraps it with `emissive_factor`
  (default zero) + emissive / tangent-space normal / occlusion
  texture slots, an `AlphaMode` (`Opaque` default, `Mask { cutoff }`
  binary at the inclusive cutoff, `Blend` clamped — resolved by
  `coverage(alpha)`), and `double_sided`. Texture slots are opaque
  `TextureBinding`s (index into a caller-managed texture table +
  `TEXCOORD` set, plus per-slot normal `scale` / occlusion
  `strength`) — the scene crate never owns texture pixels. The
  spec-defined derived BRDF inputs are methods so every consumer
  derives them identically: `diffuse_color()`
  (`base.rgb × (1 − metallic)`), `f0()`
  (`lerp(0.04, base.rgb, metallic)`), `alpha_roughness()`
  (`roughness²`), `fresnel(v_dot_h)` (per-channel Schlick).
  `is_valid()` range-checks every factor; `is_emissive()` /
  `is_textured()` / `base_coverage()` cover the common consumer
  branches. `Scene::materials: Vec<Material>` is the default-empty
  palette (helpers `push_material` / `has_materials`;
  `Scene::merge` concatenates verbatim — nothing references entries
  by index yet, mesh objects carrying a material index are a
  follow-up). The 2D `RasterRenderer` ignores the palette; like
  `Light` before it, the type is the landing place for 3D-scene
  importers / writers.
- No `oxideav-codec` or container integration yet — that comes after
  the render pipeline is real.

## Data model

### Scene

```rust
pub struct Scene {
    pub canvas: Canvas,               // pixel dims OR a vector-coord PDF page
    pub duration: SceneDuration,      // Finite(dur) | Indefinite (streaming)
    pub time_base: TimeBase,          // rational tick granularity
    pub framerate: Rational,          // output render cadence (e.g. 30/1, 24000/1001)
    pub sample_rate: u32,             // audio rate for the mix bus
    pub background: Background,       // solid colour / image / gradient / transparent
    pub objects: Vec<SceneObject>,    // z-ordered painter's algorithm
    pub audio: Vec<AudioCue>,         // triggered by timeline position
    pub metadata: Metadata,           // author / title / colour-space hints
    pub pages: Option<Vec<Page>>,     // Some(_) → paged-content mode (PDF / TIFF / EPUB)
    pub lights: Vec<LightInstance>,   // 3D punctual lights for scenes carrying 3D content
    pub materials: Vec<Material>,     // PBR material palette for 3D round-trips
}
```

A scene is addressed in its own `time_base` — same rational type oxideav
uses everywhere. `framerate` is separate: `time_base` sets the tick
granularity of every scheduled event (keyframe, lifetime, audio cue
trigger); `framerate` sets the cadence at which the renderer samples the
scene and emits frames to a sink. A scene at `time_base = 1/1000` (ms)
and `framerate = 30/1` renders at `t = 0, 33, 66, 100, …` ms. Videos
included via `ObjectKind::Video` are retimed by the renderer so their
per-frame PTS aligns with this cadence.

`SceneDuration::Indefinite` signals a streaming scene: no end, no
rewinding, the composition is driven forward by wall-clock time +
operation messages.

### Canvas

```rust
pub enum Canvas {
    /// Pixel-based raster canvas. NLE + streaming compositor use this.
    Raster { width: u32, height: u32, pixel_format: PixelFormat },
    /// Unit-agnostic vector canvas. PDF pages use this — the unit is
    /// whatever the producer declared (pt, mm, px). All coordinates
    /// inside the scene live in this unit; rasterisation happens at
    /// export time.
    Vector { width: f32, height: f32, unit: LengthUnit },
}
```

Keeping both raster and vector under one type lets the same
`SceneObject`/`Animation`/`Transform` primitives drive PDFs,
compositor streams, and NLE timelines without forking the API.

### SceneObject

```rust
pub struct SceneObject {
    pub id: ObjectId,                 // stable across edits/operations
    pub kind: ObjectKind,             // what it IS
    pub transform: Transform,         // where it is, right now (base state)
    pub lifetime: Lifetime,           // [start, end) in scene time
    pub animations: Vec<Animation>,   // per-property keyframe tracks
    pub z_order: i32,                 // painter's algorithm tie-break
    pub opacity: f32,                 // 0.0..=1.0 base opacity
    pub blend_mode: BlendMode,        // normal, multiply, screen, …
    pub effects: Vec<Effect>,         // filter chain (blur, colour shift, …)
    pub clip: Option<ClipRect>,       // geometric clipping region
}
```

### ObjectKind

```rust
pub enum ObjectKind {
    /// Static bitmap — PNG/JPEG/raw, decoded upstream into a VideoFrame.
    Image(ImageSource),

    /// Video stream — consumed as a `Packet` iterator + decoder. The
    /// scene's clock drives the stream's PTS; seeking is handled by
    /// the underlying demuxer if it supports it.
    Video(VideoSource),

    /// Styled text run. Preserves font / size / weight / colour
    /// metadata so PDF export can emit real text strings and NLE /
    /// compositor rasterise through a text-shaping backend.
    Text(TextRun),

    /// Vector shape — rect, rounded rect, polygon, bezier path.
    Shape(Shape),

    /// Container object. Applies its own `Transform` before children.
    Group(Vec<ObjectId>),

    /// Live feed from an external source (RTMP input, camera, etc.).
    /// Packets arrive asynchronously; the compositor uses the most
    /// recent frame available at render time.
    Live(LiveStreamHandle),
}
```

`ImageSource` / `VideoSource` / `LiveStreamHandle` own the heavy
resources — e.g. a `VideoSource` holds a demuxer + decoder pair, so
copying a `SceneObject` is cheap but cloning the underlying pixels
requires `Arc`-shared frame storage (managed by oxideav-core).

### Transform + Animation

```rust
pub struct Transform {
    pub position: (f32, f32),   // canvas units
    pub scale: (f32, f32),      // 1.0 = natural size
    pub rotation: f32,          // radians, around anchor
    pub anchor: (f32, f32),     // 0.0..=1.0 normalised pivot
    pub skew: (f32, f32),       // radians (Premiere-style)
}

pub struct Animation {
    pub property: AnimatedProperty,
    pub keyframes: Vec<Keyframe>,   // time-sorted
    pub easing: Easing,             // segment-level default
    pub repeat: Repeat,             // once / loop / ping-pong
}

pub enum AnimatedProperty {
    Position, Scale, Rotation, Opacity, Skew, Anchor,
    EffectParam { effect_idx: usize, param: &'static str },
    Custom(String),  // SceneObjectContent defines semantics
}

pub enum Easing {
    Linear, EaseIn, EaseOut, EaseInOut,
    CubicBezier(f32, f32, f32, f32),  // CSS / AE compatible
    Step(usize),                      // N stepped frames
    Hold,                             // no interpolation — discrete
}
```

Keyframe values are typed per property (`Vec2`, `f32`, colour, etc.)
via a `KeyframeValue` enum that `interpolate(a, b, t, easing)` acts on.

### Geometry queries

The transform layer composes upward into per-object and scene-wide
AABB accessors so layout, selection, and culling layers don't need
to walk the object list by hand:

- `Shape::content_size()` / `ObjectKind::content_size()` /
  `SceneObject::content_size()` report the object-local
  `(width, height)` for the kinds that carry one intrinsically
  (`Vector`'s viewport, `Shape::Rect` / `Shape::Polygon` AABB,
  `Live`'s `hint_size`). Image, video, text and group return
  `None` — those extents come from the renderer, not the model.
- `SceneObject::bbox(fallback)` returns the object's AABB in
  canvas space: intrinsic content size when known, the
  caller-supplied `fallback` otherwise; piped through
  `Transform::bbox` and then intersected with `ClipRect` if the
  object carries one (zero extent on no overlap so callers can
  cull).
- `Scene::bbox_at(t, fallback)` is the union AABB of every
  live object at scene time `t` — `None` for an empty / fully-
  dead scene, otherwise the smallest axis-aligned box enclosing
  every contributing object footprint. Dead and clipped-out
  objects are skipped so they don't pull the union to their
  corners. Geometric footprint only — opacity, blend mode, and
  effect chains are not modelled.
- `Scene::hit_test_at(t, point, fallback)` returns the
  `ObjectId` of the top-most live object whose AABB contains
  `point`. Painter's-algorithm order: higher `z_order` wins,
  ties broken by later insertion. AABB-only — a rotated rect's
  AABB contains corners the rect itself does not, so a
  per-pixel picker layered on top of this remains a follow-up.
- `SceneObject::effective_transform_at(t)` /
  `effective_opacity_at(t)` compose the object's base `Transform`
  and `opacity` with any `Position` / `Scale` / `Rotation` /
  `Skew` / `Anchor` / `Opacity` `Animation` tracks evaluated at
  `t`. Position / rotation / skew add, scale multiplies, anchor
  replaces, opacity multiplies and clamps to `0.0..=1.0`.
- `SceneObject::sample_at(t)` returns a `Sample` carrying
  `(id, z_order, transform, opacity, blend_mode, clip)` — the
  per-frame view a renderer needs without re-running the keyframe
  evaluator itself. `Scene::sampled_at(t)` produces one `Sample`
  per live object in paint order (z ascending, ties broken by
  insertion).

### AudioCue

```rust
pub struct AudioCue {
    pub trigger: TimeStamp,          // when playback starts in scene time
    pub source: AudioSource,         // file / clip / generator
    pub volume: Animation,           // animated 0.0..=1.0
    pub duck: Vec<DuckBus>,          // other cues to attenuate while playing
}
```

Audio cues mix into a single output bus per scene. The render pass
produces `(VideoFrame, AudioBuffer)` at each timestamp; the audio
buffer spans the interval `[last_render_time, this_render_time)` at
the scene's `sample_rate`.

## Rendering pipeline

```text
Scene + t  →  SceneSampler.sample_at(t)  →  RenderedFrame {
    video: Option<VideoFrame>,   // None for audio-only intervals
    audio: AudioBuffer,          // always valid, may be silence
    operations: Vec<ExportOp>,   // e.g. for PDF export: emit text run X
}
```

A `SceneRenderer` walks the `SceneObject` list in z-order, evaluating
transforms + animations at `t`, clipping against the canvas, and
compositing via the `BlendMode`. `RasterRenderer` implements this for
the vector slice today:

```rust
use oxideav_scene::{Background, Canvas, RasterRenderer, Scene, SceneRenderer};

let scene = Scene {
    canvas: Canvas::raster(1920, 1080),
    background: Background::Solid(0x101820FF),
    ..Scene::default()
};
let mut renderer = RasterRenderer::new();
renderer.prepare(&scene).unwrap();
let frame = renderer.render_at(&scene, 0).unwrap();
// frame.video is Some(VideoFrame) — an RGBA8 plane at the canvas size.
```

The renderer delegates per-object content fetching to each
`ObjectKind`'s own sampler:

## Source / Sink

A scene acts as a **source** of rendered frames. Wrap a `Scene` plus
a `SceneRenderer` in a `RenderedSource` and the resulting value
implements `SceneSource`: one `pull()` per frame at the scene's
`framerate`, timestamps auto-advanced by `1 / framerate`. Finite
scenes signal end-of-stream by returning `None`; indefinite scenes
run until externally stopped.

Consumers implement `SceneSink` — `init(&SourceFormat)` once, `push`
per frame, `finalise()` at end. The helper `drive(source, sink)` runs
the pull loop:

```rust
use oxideav_scene::{drive, RenderedSource, NullSink, StubRenderer, Scene};

let scene = Scene {
    framerate: oxideav_core::Rational::new(30, 1),
    ..Scene::default()
};
let mut src = RenderedSource::new(scene, StubRenderer);  // real renderer goes here
let mut sink = NullSink::default();
// drive(&mut src, &mut sink)?;  // when the real renderer lands
```

Downstream crates provide the real sinks — an `oxideav-scene-encode`
sink that pipes frames into an encoder + muxer, an
`oxideav-scene-rtmp` sink that writes to an RTMP endpoint, a
`WindowSink` for live preview, etc. Any of these can slot in without
changing the scene or renderer.

## Automatic pixel-format adaptation

Pixel formats get handled transparently in two places:

**Inbound** (source → scene): a `Video` / `Image` / `Live` object's
source frames can be in any pixel format the decoder produces —
YUV420P, YUV444P, BGRA, RGB24, NV12, whatever. The renderer converts
them to the canvas's pixel format before compositing via
[`adapt_frame_to_canvas`]. Writers of per-object samplers call this
once on each pulled frame; canvases that don't declare a raster
format (vector canvases for PDF export) short-circuit the conversion.

**Outbound** (scene → sink): when a sink expects a pixel format that
differs from the scene's canvas — e.g. a JPEG writer wants RGB24
while the scene composes in YUV420P — wrap the source in
[`AdaptedSource`]:

```rust
use oxideav_scene::{AdaptedSource, RenderedSource, Scene, StubRenderer};
use oxideav_core::PixelFormat;

let scene = Scene::default();                     // canvas: Yuv420P
let src = RenderedSource::new(scene, StubRenderer);
let adapted = AdaptedSource::new(src, PixelFormat::Rgba);
// adapted.format().canvas now reports Rgba; pulled frames are
// transparently converted on the way out.
```

Both paths delegate to [`oxideav-pixfmt`](https://crates.io/crates/oxideav-pixfmt)
— the same conversion matrix used across oxideav.

[`adapt_frame_to_canvas`]: https://docs.rs/oxideav-scene/latest/oxideav_scene/adapt/fn.adapt_frame_to_canvas.html
[`AdaptedSource`]: https://docs.rs/oxideav-scene/latest/oxideav_scene/adapt/struct.AdaptedSource.html


- `Image` samplers hold a cached decoded `VideoFrame`.
- `Video` samplers advance their demuxer/decoder to the requested PTS
  and return the most recent frame.
- `Text` samplers shape glyphs via a pluggable `TextShaper` trait
  (default: a minimal monospace fallback; real layout engines land as
  separate crates).
- `Shape` samplers rasterise on demand via a pure-Rust vector
  rasteriser (planned as `oxideav-rasterise`, another follow-up).

## Use cases in detail

### PDF pages

Each page becomes a `Scene` with `Canvas::Vector { unit: Pt, width,
height }` and one `SceneObject` per glyph run, image, and vector path.
The scene's `duration` is `Finite(1 frame)`. Edits (redact a region,
drop a watermark, rewrap a column) happen on the scene graph. When the
user re-exports:

- **PDF out** — the `SceneRenderer` walks the tree and emits PDF
  operators (`Tj` for text, `Do` for images, `f`/`S` for vectors),
  preserving structure. Text remains selectable, hyperlinks survive,
  bookmarks stay intact.
- **PNG / JPEG out** — the renderer rasterises at a requested DPI.

### Streaming compositor (RTMP server)

A daemon holds one `Scene` per live channel with `duration:
Indefinite`. A control-plane protocol (JSON over WebSocket, say)
surfaces:

```json
{"op": "add_object", "id": "lower-third", "kind": {"image": "...base64..."}, "transform": {...}}
{"op": "animate", "id": "lower-third", "property": "position", "from": [0, 0], "to": [200, 0], "duration_ms": 800, "easing": "ease_out"}
{"op": "remove_object", "id": "lower-third", "delay_ms": 5000}
```

The compositor renders the scene into a VP9/AV1/H.264 encoder fed to
an RTMP muxer. Viewers receive a normal stream; the producer only
sees the DSL.

### NLE timeline (Premiere / Resolve style)

Tracks are `SceneObject::Group` children with a shared z-order band.
Transitions between clips are implemented as opacity / position
animations that overlap two `Video` objects. Effects are the
`effects: Vec<Effect>` vector on each object. Scrubbing + preview
works by driving the `SceneSampler` at arbitrary timestamps; export
renders the entire duration at the target framerate.

## Crate layout (scaffold today)

```
src/
├── lib.rs           — module exports + Scene / Canvas root types
├── object.rs        — SceneObject + ObjectKind + Transform + BlendMode
├── animation.rs     — Animation + Keyframe + Easing + interpolation
├── audio.rs         — AudioCue + AudioSource
├── audio_mix.rs     — mix_cues(): AudioCue list → mono f32 buffer
├── render.rs        — SceneRenderer + SceneSampler traits + StubRenderer
├── raster_renderer.rs — RasterRenderer: concrete SceneRenderer (bg + shapes + vector frames → RGBA)
├── raster.rs        — rasterize_vector(): VectorFrame → VideoFrame via oxideav-raster
├── text.rs          — TextRenderer: TextRun → RGBA via oxideav-scribe + oxideav-raster
├── source.rs        — SceneSource + SceneSink + drive() + RenderedSource + NullSink / FnSink
├── adapt.rs         — pixel-format adaptation (inbound + outbound, via oxideav-pixfmt)
├── duration.rs      — SceneDuration + Lifetime
├── id.rs            — ObjectId (stable, editable)
├── ops.rs           — Operation enum for the streaming compositor
├── page.rs          — Page (paged-content / PDF mode)
├── paint.rs         — Paint + Gradient (multi-stop linear / radial)
├── light.rs         — Light / LightInstance (3D punctual lights)
├── material.rs      — Material / PbrMetallicRoughness / AlphaMode (3D PBR palette)
└── svg_path.rs      — SVG 1.1 path-data string → oxideav_core::Path
```

Everything is `pub` and `#[non_exhaustive]` on public enums so new
variants can land without an SemVer break.

## Non-goals (for now)

- **Not a vector rasteriser.** Shape rendering ships as a separate
  crate (`oxideav-rasterise`) pending.
- **Not a text shaper.** The `TextShaper` trait is pluggable; a real
  shaper lands in `oxideav-text` (pending).
- **Not an NLE UI.** This crate is the data model + renderer core; the
  UI is downstream.
- **Not a document parser.** PDF / SVG ingest land in `oxideav-pdf` /
  `oxideav-svg` (both pending) and produce `Scene`s.

## License

MIT — same as the rest of oxideav.
