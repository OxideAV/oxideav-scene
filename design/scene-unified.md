# Scene Unified Design

## Summary

The current `oxideav-scene` scaffold already reaches cleanly across the three target workloads in its most load-bearing primitives — `Canvas` already encodes raster-vs-vector, `Animation` + `Keyframe` are generic, `Operation` already sketches a mutation DSL, and the source/sink abstraction in `source.rs` is use-case-agnostic. The rest of the scaffold leans toward the live-compositor case: `Scene::objects: Vec<SceneObject>` is flat (no tracks, no layers, no pages), `SceneDuration` is a two-variant enum that cannot express a bounded-finite NLE export window alongside a live stream, and `Lifetime` is intrinsically time-based rather than "range-on-an-axis" (pages, frames, wall-clock).

The principal tension the unified model must resolve is that PDF, compositor, and NLE all compose the same objects against _different axes_ — PDF against a paged-document axis, compositor against unbounded wall-clock, NLE against a bounded timecoded axis — and all mutate the same primitives via _different vocabularies_ — compositor `Operation`s, NLE edit history, and PDF's "author composes once" workflow. If we abstract the axis and the mutation stream, the rest of the model (objects, transforms, animations, sources, effects) can stay generic across the three.

This doc proposes a unified `Axis` type replacing `SceneDuration`, a unified `Container` hierarchy replacing the flat `objects` vec (tracks/layers/pages are all `Container` kinds), a `Source` trait with a flat variant enum replacing the split `ImageSource` / `VideoSource` / `LiveStreamHandle` / `AudioSource`, and a single `SceneOp` enum that serves compositor mutations, NLE edit history, and (no-op by default) PDF authoring. The renderer trait splits into a small `Compose` core plus three capability traits — `RasterRender`, `VectorExport`, `LiveDrive` — that different concrete renderers implement à la carte. Migration is mostly additive: most current type names survive, with two significant renames (`SceneDuration` → `Axis`, `Lifetime` → `Range`) and one structural shift (`objects: Vec<SceneObject>` → `root: Container`).

## Scaffold audit

| Existing type | Fits well in | Fits awkwardly in | File:line |
|---|---|---|---|
| `Scene` | Compositor (wall-clock scene), NLE (project root) | PDF (flat `objects` is wrong for multi-page) | `src/scene.rs:10-34` |
| `Canvas::Raster` / `Canvas::Vector` | All three — raster for compositor/NLE, vector for PDF | None — clean split | `src/object.rs:18-34` |
| `SceneDuration::Finite` | NLE (export window), compositor (bounded events) | PDF (pages ≠ time), compositor (Indefinite is only choice) | `src/duration.rs:12-24` |
| `SceneDuration::Indefinite` | Compositor | NLE, PDF | `src/duration.rs:22` |
| `Lifetime { start, end }` | NLE (clip in/out), compositor (object lifetime) | PDF (should be "pages [a..b]") | `src/duration.rs:49-53` |
| `SceneObject` | All three — identity + transform + animations + effects | None (but `kind` variants push toward one case) | `src/object.rs:73-85` |
| `ObjectKind::Image/Video/Text/Shape` | All three | None | `src/object.rs:106-114` |
| `ObjectKind::Live` | Compositor only | NLE, PDF | `src/object.rs:113` |
| `ObjectKind::Group(Vec<ObjectId>)` | NLE (tracks), PDF (nested content) | Represented as IDs not a real tree — layout/mutation story fragile | `src/object.rs:112` |
| `Transform` (px units) | Compositor, NLE | PDF (needs points) — unit is implicit, lives in `Canvas` not `Transform` | `src/object.rs:118-129` |
| `Effect { name, params: Vec<(String, f32)> }` | Compositor, PDF (colour-adjust) | NLE (needs richer typed params + keyframable via `EffectParam`) | `src/object.rs:169-173` |
| `Animation` / `Keyframe` / `Easing` | All three (the cleanest shared primitive in the crate) | None | `src/animation.rs:12-18` |
| `AudioCue` | Compositor (sound cues), NLE (audio track clips) | PDF (no audio) — but PDF can ignore | `src/audio.rs:15-27` |
| `AudioSource` (enum) | Compositor, NLE | Duplicates `ImageSource`/`VideoSource` structure — should share a `Source` hierarchy | `src/audio.rs:31-50` |
| `ImageSource` / `VideoSource` / `LiveStreamHandle` | Each fits one case | Three disjoint types for what is really one "Source" concept | `src/object.rs:187-279` |
| `Operation` (mutation DSL) | Compositor | NLE (missing ripple/roll/slip/slide/split), PDF (n/a) | `src/ops.rs:20-54` |
| `ExportOp` (output DSL) | PDF (vector ops), compositor (raster emit) | NLE — adequate but no per-clip cache hook | `src/ops.rs:62-83` |
| `SceneRenderer::render_at` | NLE (frame export), PDF (single frame) | Compositor (needs wall-clock drive, back-pressure) | `src/render.rs:29-42` |
| `SceneSource` / `SceneSink` / `drive()` | Compositor, NLE export | PDF (one-shot — `drive` works, but framerate semantic is weird) | `src/source.rs:67-100` |
| `RenderedSource::scene_mut` | Compositor | NLE (wants edit history / undo, not raw mutation) | `src/source.rs:161-163` |
| `framerate: Rational` | Compositor (output cadence), NLE (project fps) | PDF (not really a "rate" — pages are discrete) | `src/scene.rs:25` |
| `Metadata` | PDF (doc info), NLE (project metadata) | Compositor (stream metadata) | `src/scene.rs:137-147` |

## Cross-cutting tensions and resolutions

1. **Duration vs axis.** `SceneDuration::{Finite,Indefinite}` collapses three distinct "axes of progression" into a boolean. PDF progresses by page, compositor by wall-clock, NLE by timecode-bounded-range. **Resolve:** introduce `Axis` with four variants — `Instant` (PDF single page still), `Paged(count)` (PDF multi-page), `Range { start, end }` (NLE export window or any bounded segment), `Unbounded { epoch }` (compositor wall-clock from an epoch). Keep `TimeStamp = i64` in `time_base` ticks for all cases; pages are integer ticks on a page-tick time base (`1/1` page-per-unit).

2. **Flat objects vs tracks vs layers vs pages.** Compositor wants a scene graph, NLE wants tracks, PDF wants pages. **Resolve:** replace `objects: Vec<SceneObject>` with `root: Container`, where `Container::kind` is `Stage` (root, always exactly one), `Page { index, media_box }` (PDF pages), `Track { kind: Video|Audio|Subtitle, index }` (NLE), `Layer { z_band: (i32, i32) }` (compositor logical layer), or `Group { transform: Transform }` (everything else). A `Container` has `children: Vec<Node>` where `Node = Container(Box<Container>) | Leaf(SceneObject)`. The tree preserves ordering, z-banding, and provenance.

3. **Static vs mutable state.** PDF is "author once and export", compositor is live mutation, NLE is mutation + undo. **Resolve:** `Scene` is always a pure data structure; mutation is always done via a `SceneOp` applied by an `OpInterpreter`. For live compositor, `OpInterpreter` consumes from a queue; for NLE, `OpInterpreter` appends to an `OpLog` with undo/redo; for PDF, the author builds the scene once — they can still go through `OpInterpreter` but typically use builder APIs.

4. **`Lifetime` vs clip in/out vs page range.** They are all "this thing exists between two points on the axis". **Resolve:** rename `Lifetime` to `Range<A: Axis>`; NLE clips use `Range<Time>`, PDF objects use `Range<Page>` (appears on pages a..b), compositor objects use `Range<Time>` with `end: Option<TimeStamp>`. One type, generic over the axis variant.

5. **Keyframed animations — same type, different interpretations.** Already clean. `Animation` over `TimeStamp` works for all three. PDF uses `Animation::sample(0)` always (single-instant scenes) — harmless.

6. **`Source` abstraction.** Currently split across four types with redundant `Path`/`EncodedBytes` variants. **Resolve:** unify into one `Source` enum with a `MediaKind` tag (`Image`|`Video`|`Audio`|`Live`|`NestedScene`) and shared variants (`File`, `Bytes`, `Decoded`, `Live`, `Generator`, `Proxy(low_res, full)`, `NestedScene(Arc<Scene>)`). Add `Source::resolve(t: TimeStamp) -> Result<Sample>` to trait.

7. **Effect chains vs single Effect.** NLE wants per-clip effect chains with typed parameters; compositor just wants a filter list. Both are "Vec<Effect>" already; the type is fine. **Resolve:** keep `Vec<Effect>` on `SceneObject`, but widen `Effect::params` from `Vec<(String, f32)>` to `EffectParams` (a key→`KeyframeValue` map) so `EffectParam` keyframes remain well-typed.

8. **Mutation ops vs edit ops.** Compositor `Operation` covers add/remove/animate; NLE needs ripple/roll/slip/slide/split. **Resolve:** one `SceneOp` enum with a "scene graph" layer (add/remove/move/setprop) and a "timeline" layer (ripple/roll/slip/slide/split/insert/overwrite) that are defined _in terms of_ graph ops. NLE records high-level edits and expands them at replay time; compositor sends low-level graph ops directly. Both go through the same `OpInterpreter`.

9. **Units and transforms.** Currently `Transform` is unitless and `Canvas` carries the unit. This is fine as long as `Transform::position` is interpreted in the enclosing canvas's unit — but there's no enforcement. **Resolve:** keep `Transform` unitless but document the invariant; add `Length { value: f32, unit: LengthUnit }` helper for cases where an external type (e.g. PDF media box) needs a unit-tagged value. Normalized-to-canvas uses `LengthUnit::Normalized` (new variant, 0..=1 of containing canvas). NLE can use pixel or normalized interchangeably.

10. **Deterministic replay.** Compositor must replay an op-log + live-source log bit-exactly. **Resolve:** every `SceneOp` carries a `seq: u64` (monotonic) and an `at: TimeStamp` (scene-time the op applies). Simultaneous ops break ties by `seq`. Seeded PRNG lives on the `Scene` (field `seed: u64`); effects pull from a deterministic sub-stream indexed by `ObjectId`.

11. **Render API taxonomy.** One trait ≠ enough — rasterising a still, exporting PDF ops, driving a live encoder, and pre-computing NLE cache are four different things. **Resolve:** a tiny core `Compose` trait (produce an abstract `ComposedFrame` at time `t`) plus three capability traits — `RasterRender : Compose`, `VectorExport : Compose`, `LiveDrive : Compose` — that concrete renderers implement à la carte. `SceneRenderer` in the current sense becomes an alias for "implements `RasterRender`".

## Proposed unified types

### Axis + time primitives (replaces `SceneDuration`)

```rust
// Serves: PDF (Instant, Paged), NLE (Range), compositor (Unbounded).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Instant,                                      // PDF single page
    Paged { count: u32 },                         // PDF multi-page
    Range { start: TimeStamp, end: TimeStamp },   // NLE export window / bounded clip
    Unbounded { epoch: TimeStamp },               // compositor wall-clock origin
}

impl Axis {
    pub fn contains(&self, t: TimeStamp) -> bool { /* ... */ }
    pub fn end(&self) -> Option<TimeStamp> { /* ... */ }
    /// Total ticks on the axis; None for Unbounded.
    pub fn extent(&self) -> Option<TimeStamp> { /* ... */ }
}

// Serves: NLE (clip lifetime), compositor (object lifetime), PDF (page range).
// Replaces `Lifetime`. Same on-disk shape — just a rename.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: TimeStamp,
    pub end: Option<TimeStamp>,
}
```

Justification: every use case has a "progression axis" and "per-object range on that axis". Collapsing them into a unified pair is the minimum-viable abstraction.

### Container hierarchy (replaces flat `Vec<SceneObject>`)

```rust
// Serves: PDF (pages), NLE (tracks), compositor (layers), all three (groups).
#[derive(Clone, Debug)]
pub struct Container {
    pub id: ObjectId,
    pub kind: ContainerKind,
    pub transform: Transform,           // applied before children
    pub range: Range,                   // when this container is active on the axis
    pub children: Vec<Node>,
    pub effects: Vec<Effect>,           // container-level effects (NLE "adjustment layer")
}

#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ContainerKind {
    Stage,                              // unique root
    Page { index: u32, media_box: (f32, f32, LengthUnit) },
    Track { kind: TrackKind, index: u32 },
    Layer { z_band: (i32, i32) },
    Group,
    NestedSequence(Arc<Scene>),         // NLE nested sequence, PDF form XObject
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind { Video, Audio, Subtitle, Effects }

#[derive(Clone, Debug)]
pub enum Node {
    Container(Box<Container>),
    Leaf(SceneObject),
}
```

Justification: PDF needs pages-as-containers; NLE needs tracks-as-containers; compositor uses `Group`/`Layer` as containers for scene graph. The same recursion fits all three. NestedSequence covers NLE nested sequences and PDF form XObjects with one variant.

### Source trait (replaces `ImageSource` + `VideoSource` + `LiveStreamHandle` + `AudioSource`)

```rust
// Serves: all three — file (PDF image, NLE clip), live (compositor), decoded, nested.
pub trait Source: Send + Sync + std::fmt::Debug {
    fn kind(&self) -> MediaKind;
    fn natural_size(&self) -> Option<(f32, f32)>;
    fn resolve(&self, t: TimeStamp) -> Result<Sample>;
    fn is_live(&self) -> bool { false }
    /// NLE proxy support.
    fn proxy(&self) -> Option<&dyn Source> { None }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind { Image, Video, Audio, Live, NestedScene, Vector, Text }

// Flat variant enum for serde + cheap clone.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum SourceSpec {
    File { path: String, kind: MediaKind, proxy: Option<Box<SourceSpec>> },
    Bytes { data: Arc<[u8]>, kind: MediaKind },
    DecodedFrame(Arc<oxideav_core::VideoFrame>),
    DecodedAudio { sample_rate: u32, channels: u8, samples: Arc<[f32]> },
    Live { uri: String, hint_size: Option<(u32, u32)>, hold_ms: u32 },
    Generator(Generator),
    NestedScene(Arc<Scene>),            // compositor scene-in-scene, NLE nested seq
}
```

Justification: the current four source types have 80% variant overlap. Consolidating into `SourceSpec` (data) + `Source` (trait for live resolution) means compositor / NLE / PDF all use the same type when loading assets.

### Object kind (narrowed — content lives in `Source`)

```rust
// Serves: all three. Previous `ObjectKind` had both kind and payload mingled;
// this splits them so Source owns the payload and Kind is a pure discriminant.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ObjectKind {
    Media(Arc<dyn Source>),             // image/video/live/nested — polymorphic via Source
    Text(TextRun),                       // structural (PDF wants to keep this typed)
    Shape(Shape),                        // vector (PDF wants to keep this typed)
}
```

Justification: `Text` and `Shape` stay first-class because PDF's vector export needs to inspect them structurally. Everything else collapses to `Media` with a `Source`.

### SceneObject (unchanged shape, slight edits)

```rust
// Serves: all three. Same fields as today, two changes:
// - `lifetime: Lifetime` → `range: Range` (rename).
// - `effects: Vec<Effect>` effect params now typed (see below).
#[derive(Clone, Debug)]
pub struct SceneObject {
    pub id: ObjectId,
    pub kind: ObjectKind,
    pub transform: Transform,
    pub range: Range,
    pub animations: Vec<Animation>,
    pub z_order: i32,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub effects: Vec<Effect>,
    pub clip: Option<ClipRect>,
}

#[derive(Clone, Debug)]
pub struct Effect {
    pub name: String,
    pub params: EffectParams,           // replaces Vec<(String, f32)>
}

pub type EffectParams = std::collections::BTreeMap<String, KeyframeValue>;
```

### Scene (root)

```rust
// Serves: all three.
#[derive(Clone, Debug)]
pub struct Scene {
    pub canvas: Canvas,
    pub axis: Axis,                     // replaces SceneDuration
    pub time_base: TimeBase,
    pub framerate: Rational,            // compositor/NLE; PDF ignores except for embedded video
    pub sample_rate: u32,
    pub background: Background,
    pub root: Container,                // replaces `objects: Vec<SceneObject>`
    pub audio: Vec<AudioCue>,
    pub markers: Vec<Marker>,           // NLE markers + PDF chapters + compositor cue points
    pub metadata: Metadata,
    pub seed: u64,                      // deterministic replay
}

#[derive(Clone, Debug)]
pub struct Marker {
    pub id: ObjectId,
    pub at: TimeStamp,
    pub name: String,
    pub kind: MarkerKind,               // Chapter | Cue | Note
}
```

Justification: `markers` serves NLE markers, PDF chapters/bookmarks, and compositor cue points. `seed` exists for the compositor but is harmless to the others.

## Proposed trait hierarchy

```rust
// Core — every renderer produces an abstract composed description.
pub trait Compose {
    fn prepare(&mut self, scene: &Scene) -> Result<()>;
    fn compose_at(&mut self, scene: &Scene, t: TimeStamp) -> Result<ComposedFrame>;
    fn seek(&mut self, t: TimeStamp) -> Result<()>;
}

pub struct ComposedFrame {
    pub draws: Vec<DrawCommand>,        // z-ordered, transform-flattened
    pub audio: Vec<f32>,                // mixed bus since last call
    pub timestamp: TimeStamp,
}

// Capability 1 — rasterise to pixels (NLE export, PDF→PNG, compositor preview).
pub trait RasterRender: Compose {
    fn raster_at(&mut self, scene: &Scene, t: TimeStamp) -> Result<VideoFrame>;
}

// Capability 2 — emit vector ops (PDF structural export).
pub trait VectorExport: Compose {
    fn export_vector_at(&mut self, scene: &Scene, t: TimeStamp) -> Result<Vec<ExportOp>>;
}

// Capability 3 — drive forward from wall-clock (RTMP compositor).
pub trait LiveDrive: Compose {
    fn advance_to_wallclock(&mut self, scene: &mut Scene, now: TimeStamp,
                            ops: &mut dyn Iterator<Item = SceneOp>) -> Result<ComposedFrame>;
    fn frame_deadline(&self) -> std::time::Duration;
}
```

Justification: a PDF exporter implements `Compose + VectorExport + RasterRender` (for rasterising). An NLE exporter is `Compose + RasterRender`. A compositor is `Compose + RasterRender + LiveDrive`. Each capability trait justifies itself from ≥ 2 concrete implementations.

## Unified `SceneOp` vocabulary

```rust
// One enum, all three use cases.
// Compositor: sends Graph ops over the wire.
// NLE: records Timeline ops in the op-log and expands to Graph ops on replay.
// PDF: author uses builder APIs; replay is a no-op since scene is authored once.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct SceneOp {
    pub seq: u64,
    pub at: TimeStamp,
    pub op: OpKind,
}

#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum OpKind {
    // --- Graph layer (primitive; compositor uses directly) ---
    AddNode { parent: ObjectId, node: Box<Node> },
    RemoveNode { id: ObjectId },
    ReparentNode { id: ObjectId, new_parent: ObjectId, index: usize },
    SetTransform { id: ObjectId, transform: Transform },
    SetProperty { id: ObjectId, prop: PropertyPath, value: KeyframeValue },
    AddAnimation { id: ObjectId, animation: Animation },
    CancelAnimation { id: ObjectId, property: AnimatedProperty },
    FireAudio(Box<AudioCue>),
    AddMarker(Marker),
    RemoveMarker(ObjectId),
    EndScene,

    // --- Timeline layer (NLE-high-level; desugars to Graph ops) ---
    Ripple { track: ObjectId, at: TimeStamp, delta: TimeStamp },
    Roll { left: ObjectId, right: ObjectId, delta: TimeStamp },
    Slip { id: ObjectId, delta: TimeStamp },
    Slide { id: ObjectId, delta: TimeStamp },
    Split { id: ObjectId, at: TimeStamp },
    Insert { track: ObjectId, at: TimeStamp, node: Box<Node> },
    Overwrite { track: ObjectId, range: Range, node: Box<Node> },

    // --- Session layer (undo/redo, only meaningful in OpLog context) ---
    GroupBegin { label: String },
    GroupEnd,
}

/// PropertyPath identifies any tweakable scalar on any node. Used by
/// SetProperty and by AnimatedProperty for uniformity.
#[derive(Clone, Debug)]
pub enum PropertyPath {
    Opacity,
    BlendMode,
    ZOrder,
    Transform(TransformField),
    EffectParam { effect_idx: usize, param: String },
    Custom(String),
}
```

Justification: compositor only ever emits Graph + Session ops; NLE emits Timeline ops that the interpreter desugars; PDF emits nothing at runtime but can replay a prebuilt op-log to reconstruct a scene. One enum, three dispatch tables.

## Serialisation sketch

Canonical JSON for a scene that mixes all three workloads — a live compositor whose inputs include a multi-page PDF as a `NestedScene` source and an NLE sequence as another nested scene:

```json
{
  "canvas": { "raster": { "width": 1920, "height": 1080, "pixel_format": "Yuv420P" } },
  "axis": { "unbounded": { "epoch": 0 } },
  "time_base": { "num": 1, "den": 1000 },
  "framerate": { "num": 30, "den": 1 },
  "sample_rate": 48000,
  "seed": 42,
  "root": {
    "id": 1,
    "kind": { "stage": {} },
    "transform": "identity",
    "range": { "start": 0, "end": null },
    "children": [
      { "container": {
          "id": 2,
          "kind": { "layer": { "z_band": [0, 99] } },
          "children": [
            { "leaf": {
                "id": 100,
                "kind": { "media": { "nested_scene": "ref:pdf_scene_001" } },
                "range": { "start": 0, "end": 10000 },
                "transform": { "position": [0, 0], "scale": [1, 1] },
                "z_order": 10
            }},
            { "leaf": {
                "id": 101,
                "kind": { "media": { "live": { "uri": "rtmp://cam1", "hint_size": [1280, 720], "hold_ms": 500 }}},
                "range": { "start": 0, "end": null },
                "z_order": 20
            }},
            { "leaf": {
                "id": 102,
                "kind": { "media": { "nested_scene": "ref:nle_seq_a" } },
                "range": { "start": 2000, "end": 8000 },
                "z_order": 30
            }}
          ]
      }}
    ]
  },
  "audio": [],
  "markers": [ { "id": 200, "at": 5000, "name": "goal", "kind": "cue" } ],
  "metadata": { "title": "match-7 live" },
  "nested_scenes": {
    "pdf_scene_001": { "canvas": { "vector": { } }, "axis": { "paged": { "count": 4 } } },
    "nle_seq_a":    { "canvas": { "raster": { } }, "axis": { "range": { "start": 0, "end": 6000 } } }
  },
  "op_log": [
    { "seq": 1, "at": 100, "op": { "add_animation": { "id": 101, "animation": { } } } }
  ]
}
```

Key design points for serde: `Axis` is tagged-enum; `ContainerKind` / `ObjectKind` / `SourceSpec` are tagged-enums; `Source` trait objects serialize via their underlying `SourceSpec` (which is `Clone + Serialize`); nested scenes are stored by-reference in a `nested_scenes` dictionary to avoid duplication and cycles; `op_log` is optional (only present for compositor replay + NLE undo history). A PDF scene serializes with `axis: { paged: {...} }`, empty `op_log`, and a page-per-container root — nothing else in the format is PDF-specific.

## Migration plan

**Renames (mechanical; no semantic change):**
- `SceneDuration` → `Axis` — enum widens to 4 variants; `Finite(n)` becomes `Range { start: 0, end: n }`; `Indefinite` becomes `Unbounded { epoch: 0 }`.
- `Lifetime` → `Range` — identical fields.
- `SceneObject::lifetime` → `SceneObject::range`.

**Additive (no break for existing users):**
- New `Container`, `ContainerKind`, `Node`, `TrackKind`, `Marker`, `MarkerKind`.
- New `Source` trait + `SourceSpec` enum. Old `ImageSource`/`VideoSource`/`AudioSource`/`LiveStreamHandle` become `From<X> for SourceSpec` adapters for one version, then deprecated.
- New `Compose`, `RasterRender`, `VectorExport`, `LiveDrive` traits. `SceneRenderer` stays as `trait SceneRenderer: Compose + RasterRender {}` for one version, then phased out.
- New `SceneOp` struct (with `seq`, `at`). Old `Operation` is kept as `type Operation = OpKind;` for one release.
- `PropertyPath`, `EffectParams` (`BTreeMap<String, KeyframeValue>`) — `Effect::params` gains a compatibility accessor that returns the old `Vec<(String, f32)>` shape for scalar-only params.

**Breaks (major version bump):**
- `Scene::objects: Vec<SceneObject>` → `Scene::root: Container`. Callers migrate via a helper `Scene::from_legacy_objects(objs: Vec<SceneObject>) -> Scene` that wraps them in a single `Stage → Layer → leaves` tree.
- `Scene::visible_at(t)` now walks the tree — same return type `Vec<&SceneObject>`.
- `Operation::AddObject` goes away (use `OpKind::AddNode`); `Operation::RemoveObject` becomes `OpKind::RemoveNode`.

**Suggested sequencing:**
1. Land `Axis` + `Range` (renames) behind `#[doc(hidden)] type SceneDuration = Axis;` shim — no downstream breaks.
2. Land `Container` + `Node` alongside existing `Vec<SceneObject>`; `Scene::root` is new, `Scene::objects` deprecated.
3. Land `Source` trait + `SourceSpec`; migrate `ObjectKind` to `Media(Arc<dyn Source>)` in a new `ObjectKindV2` alongside the old one, then rename.
4. Split renderer trait into `Compose` + capability traits; `SceneRenderer` auto-impl delegates.
5. Rename `Operation` → `OpKind`; add `SceneOp { seq, at, op }` wrapper; NLE-extra variants land here.
6. Drop deprecated aliases in the next major release.

## Open questions

1. **Vector-canvas cross-referencing.** Should `NestedScene(Arc<Scene>)` inside a `Canvas::Raster` scene force rasterisation of the nested scene, or can vector content flow through to a final vector-capable exporter? (Compositor will always rasterise; PDF export of an NLE-embedded-in-PDF scene is the edge case.)
2. **Axis for mixed-page-plus-time scenes.** A single PDF scene that embeds a video preview — is the outer axis `Paged(n)` and the inner nested scene `Range`, or do we need a 2D axis `PagedTime`? Current proposal: nested scene has its own axis.
3. **`Source` trait object or generic?** `Arc<dyn Source>` is serde-unfriendly (we serialize via `SourceSpec`). Are we OK with the asymmetry between "runtime polymorphism via trait object" and "storage/wire via enum"?
4. **`seed: u64` placement.** Should the seed live on `Scene` (global), on each effect (per-node), or both? Proposal assumes scene-global with per-`ObjectId` sub-streams derived via hash.
5. **Paged animation.** Do keyframes in a PDF `Paged` scene mean anything, or are animations only valid on `Range`/`Unbounded` axes? Proposal: keyframes on a paged scene interpolate by page index (useful for animated PDFs in a print-export pipeline). Needs confirmation.
6. **Transitions as first-class type?** A cross-fade is currently expressed as two overlapping `SceneObject`s with opacity animations. NLE UIs typically want a `Transition` primitive so ripple/roll edits preserve it. Proposal: add `ObjectKind::Transition { outgoing: ObjectId, incoming: ObjectId, kind: TransitionKind, duration: TimeStamp }` later, deferred to v0.2.
7. **Op-log storage on `Scene`.** Should `op_log` live inside `Scene` (canonical for replay) or outside (passed alongside)? Proposal: optional field `op_log: Option<OpLog>`.
8. **Back-pressure and frame deadlines.** `LiveDrive::frame_deadline` returns a `Duration` — but how does the renderer report missed deadlines upstream? Callback trait? Returned status on `advance_to_wallclock`? Needs interface detail.
9. **Undo granularity.** NLE `OpLog` groups ops via `GroupBegin`/`GroupEnd`. Is a flat grouped-op-log enough, or do we need hierarchical undo (undo the group, then undo a specific op inside)?
10. **`SourceSpec::File` resolution policy.** Is resolution `Source`-trait's job (object-level), or is there a scene-level `Assets` registry that resolves paths once (content-addressed)? Proposal: scene-level registry on `Scene::assets: Assets`.
