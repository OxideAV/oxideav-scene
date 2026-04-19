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

**Scaffold.** This crate ships the type model + public-API shape for
all three use cases and a placeholder `SceneRenderer` trait. No real
rendering, encoding, or file-format I/O yet — those land as follow-ups.

- `Scene`, `SceneObject`, `ObjectKind`, `Transform`, `Animation`,
  `Keyframe`, `Easing`, `AudioCue` types are in place.
- `SceneRenderer` + `SceneSampler` traits are defined but return
  `Error::Unsupported` on every call.
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
compositing via the `BlendMode`. The renderer delegates per-object
content fetching to each `ObjectKind`'s own sampler:

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
├── render.rs        — SceneRenderer + SceneSampler traits + StubRenderer
├── source.rs        — SceneSource + SceneSink + drive() + RenderedSource + NullSink / FnSink
├── duration.rs      — SceneDuration + Lifetime
├── id.rs            — ObjectId (stable, editable)
└── ops.rs           — Operation enum for the streaming compositor
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
