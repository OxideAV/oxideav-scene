# Scene Unified Design

> **Revision 2 — scope clarifications from user feedback.**
>
> - **PDF is still a Scene.** Each page is a discrete composition of
>   objects; the Scene abstraction holds. The difference is that a
>   PDF's axis is a **page index**, not a timeline, and **no object
>   survives between pages** — each page container has its own set
>   of objects. There's no interpolation between pages; moving from
>   page 2 to page 3 is a hard swap to a different object set.
> - **Embedded videos in a PDF are `VideoSource`s, not nested
>   Scenes.** An embedded video is just a media object with a
>   Source that happens to be a video stream — sampled when the
>   viewer plays it, independent of the page it lives on.
> - **Scenes are not nestable as first-class objects.** Earlier
>   drafts of this doc tried to add a `SourceSpec::NestedScene`
>   variant. User feedback: there's no compelling case.
>   Picture-in-picture is just a second `VideoSource`. Reusable
>   templates overwrite a target scene rather than being embedded.
>   For the "use an intro scene inside another scene" case, the
>   pattern is to _render_ the inner scene (to disk or to an
>   in-memory producer) and consume its output as a regular
>   `VideoSource` — no new variant needed, no renderer
>   recursion to reason about, no vector-flow-through edge case.
>   If we discover a concrete need for real nesting later, we can
>   add it; the current model intentionally doesn't include it.
> - **A Scene's "feature set" is data-driven.** A scene with no
>   video / audio / animation doesn't meaningfully have a framerate
>   or a sample rate; those fields exist in the struct but are
>   ignored. A paged scene (PDF) has no framerate semantics. A
>   compositor scene doesn't use the `Axis::Range` variant. One
>   type, many shapes.
> - **Scene-to-scene transitions are the common "transition" case**,
>   not clip-level cross-fades inside one scene. Most videos are a
>   single scene per piece of content; cross-fade / cut / wipe
>   between two scenes at the output level is the "image transition"
>   users mean. This revision adds a `Program` layer that sequences
>   full scenes with transitions between them. Clip-level cross-
>   fades inside a single scene are still expressible (for
>   splitscreen fades and overlay ramps) but they're not the primary
>   meaning of "transition".

## Summary

The current `oxideav-scene` scaffold already reaches cleanly across the three target workloads in its most load-bearing primitives — `Canvas` cleanly distinguishes raster from vector, `Animation` + `Keyframe` are generic, `Operation` sketches a mutation DSL, and the source/sink abstraction in `source.rs` is use-case-agnostic. The rest of the scaffold leans toward the live-compositor case: `Scene::objects: Vec<SceneObject>` is flat (no tracks, no layers, no pages), `SceneDuration` is a two-variant enum that cannot express a bounded NLE window alongside a live stream alongside a multi-page document, and `Lifetime` is intrinsically time-based rather than "range on the scene's axis".

The principal tension the unified model must resolve is that PDF, compositor, and NLE all compose the same objects against _different axes_ — PDF against page index, compositor against unbounded wall-clock, NLE against a bounded timecoded range — and all mutate the same primitives via _different vocabularies_ — compositor `Operation`s, NLE edit history, and PDF's author-once workflow. If we abstract the axis and the mutation stream, the rest of the model (objects, transforms, animations, sources, effects) stays generic across the three. Features that don't apply to a given scene (e.g. framerate on a paged PDF) exist in the type but have trivial values — no separate type hierarchy needed.

This doc proposes a unified `Axis` type replacing `SceneDuration` (variants: `Range`, `Unbounded`, `Paged`, `Instant`), a unified `Container` hierarchy replacing the flat `objects` vec (tracks, layers, and pages are all `Container` kinds), a `Source` trait with a flat variant enum replacing the split `ImageSource` / `VideoSource` / `LiveStreamHandle` / `AudioSource`, and a single `SceneOp` enum that serves all three mutation vocabularies. The renderer trait splits into a small `Compose` core plus capability traits — `RasterRender` (pixels), `VectorExport` (PDF ops), `LiveDrive` (wall-clock + back-pressure) — that different concrete renderers implement à la carte. A separate `Program` type sequences multiple `Scene`s with between-scene transitions (cut, cross-fade, wipe, dip-to-black). **Scenes are not nestable as a Scene variant**; when one scene needs to appear inside another (intro-scene-as-clip), the inner scene is rendered and consumed as a regular `VideoSource`. Migration is mostly additive: most current type names survive, with two significant renames (`SceneDuration` → `Axis`, `Lifetime` → `Range`) and one structural shift (`objects: Vec<SceneObject>` → `root: Container`).

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

1. **Duration vs axis.** `SceneDuration::{Finite,Indefinite}` collapses four distinct "axes of progression" into a boolean. PDF progresses by page, compositor by wall-clock, NLE by timecode-bounded-range, a single-frame export is an instant. **Resolve:** introduce `Axis` with four variants — `Instant` (single-frame), `Paged { count }` (PDF), `Range { start, end }` (NLE export window or any bounded segment), `Unbounded { epoch }` (compositor wall-clock from an epoch). `TimeStamp = i64` in `time_base` ticks for time-based axes; pages are integer indices (no interpolation between them).

2. **Flat objects vs tracks vs layers vs pages.** Compositor wants a scene graph, NLE wants tracks, PDF wants pages. **Resolve:** replace `objects: Vec<SceneObject>` with `root: Container`, where `Container::kind` is `Stage` (root, always exactly one), `Page { index }` (PDF — each page owns its own distinct set of objects; no object lives across pages), `Track { kind: Video|Audio|Subtitle, index }` (NLE), `Layer { z_band: (i32, i32) }` (compositor logical layer), or `Group { transform: Transform }` (everything else). A `Container` has `children: Vec<Node>` where `Node = Container(Box<Container>) | Leaf(SceneObject)`. Page children are reachable only when rendering that page; swapping pages is a hard scene-graph swap, not an interpolation.

3. **Static vs mutable state.** PDF is author-once-and-export; compositor is live mutation; NLE is mutation + undo. **Resolve:** `Scene` is always a pure data structure; mutation always goes through a `SceneOp` applied by an `OpInterpreter`. For live compositor, `OpInterpreter` consumes from a queue. For NLE, `OpInterpreter` appends to an `OpLog` with undo/redo. For PDF, the author typically uses builder APIs but can still go through `OpInterpreter` if desired (useful for "programmatically generate a 1000-page PDF" workflows).

4. **`Lifetime` vs clip in/out vs page membership.** Time-based scenes (NLE, compositor) have objects that live between two timecodes; PDF objects live on a specific page via the Page container, not via a time range. **Resolve:** rename `Lifetime` to `Range` and use it _only_ for time-axis scenes. PDF objects don't carry a `Range` — they're located in a specific Page container and that _is_ their lifetime. Put differently: `Range` is relevant only when `scene.axis` is `Range` or `Unbounded`.

5. **Keyframed animations.** Already clean. `Animation` over `TimeStamp` works for both NLE and compositor. PDF scenes don't use animations on static content, but nothing stops a PDF from embedding an animated SVG-style object if someone wants to (rendered at a single instant, any animation ignored).

6. **Scene-level vs program-level transitions.** Users expect "image transitions" at the boundary between two full scenes, not as clip-level effects inside one scene. Most videos are a single scene per piece of content; cross-fading between two scenes is what "transition" usually means in a higher-level program. **Resolve:** add a `Program` type that sequences `Scene`s with a `Transition` between each adjacent pair. Clip-level cross-fades inside a single scene are still expressible via opacity animations on overlapping clips (NLE uses them for splitscreen and overlay fades), but the common case — "clip 1 ends, clip 2 starts, half-second cross-fade" — lives at the Program level.

7. **`Source` abstraction.** Currently split across four types with redundant `Path`/`EncodedBytes` variants. **Resolve:** unify into one `Source` enum with a `MediaKind` tag (`Image`|`Video`|`Audio`|`Live`) and shared variants (`File`, `Bytes`, `Decoded`, `Live`, `Generator`, `Proxy(low_res, full)`). Add `Source::resolve(t: TimeStamp) -> Result<Sample>`. Notes: a video embedded in a PDF is just a `Source::File { kind: Video }` on a PDF-page `SceneObject` (playback is triggered by the viewer, not synced to the host). Picture-in-picture in a compositor is a second `Source::Live` — not a nested scene. The intro-scene-as-clip case renders the inner scene to produce a video stream that's consumed here as a regular `Source::File { kind: Video }` (or an in-memory `Source::Generator`).

8. **Effect chains vs single Effect.** NLE wants per-clip effect chains with typed parameters; compositor just wants a filter list. Both are "Vec<Effect>" already; the type is fine. **Resolve:** keep `Vec<Effect>` on `SceneObject`, but widen `Effect::params` from `Vec<(String, f32)>` to `EffectParams` (a key→`KeyframeValue` map) so `EffectParam` keyframes remain well-typed.

9. **Mutation ops vs edit ops.** Compositor `Operation` covers add/remove/animate; NLE needs ripple/roll/slip/slide/split. **Resolve:** one `SceneOp` enum with a "scene graph" layer (add/remove/move/setprop) and a "timeline" layer (ripple/roll/slip/slide/split/insert/overwrite) that are defined _in terms of_ graph ops. NLE records high-level edits and expands them at replay time; compositor sends low-level graph ops directly. Both go through the same `OpInterpreter`.

10. **Units and transforms.** `Transform` is unitless; the enclosing `Canvas` carries the unit. **Resolve:** keep `Transform` unitless; document the invariant. For NLE + compositor, pixel units are the default. A future `LengthUnit::Normalized` (0..=1 of containing canvas) can land alongside without changing `Transform`.

11. **Deterministic replay.** Compositor must replay an op-log + live-source log bit-exactly. **Resolve:** every `SceneOp` carries a `seq: u64` (monotonic) and an `at: TimeStamp` (scene-time the op applies). Simultaneous ops break ties by `seq`. Seeded PRNG lives on the `Scene` (field `seed: u64`); effects pull from a deterministic sub-stream indexed by `ObjectId`.

12. **Render API taxonomy.** One trait isn't enough — rasterising a frame, exporting PDF vector ops, pre-computing an NLE cache tile, and driving a live encoder with wall-clock deadlines are different surfaces. **Resolve:** a tiny core `Compose` trait (produce an abstract `ComposedFrame` at time/page `t`) plus three capability traits — `RasterRender : Compose` (pixels), `VectorExport : Compose` (PDF content-stream ops), `LiveDrive : Compose` (wall-clock + back-pressure) — that concrete renderers implement à la carte. A PDF exporter is `Compose + VectorExport`; a PDF→PNG rasteriser is `Compose + RasterRender`. An NLE exporter is `Compose + RasterRender`. A compositor is `Compose + RasterRender + LiveDrive`.

13. **Scenes-inside-scenes.** The intro-scene-at-the-start-of-every-broadcast case needs solving, but not via a `SourceSpec::NestedScene` variant. **Resolve:** a `Scene` can _produce_ a video stream via its renderer; any consumer that wants to use that scene elsewhere loads the produced stream as a `VideoSource` (either via a temp file or an in-memory producer trait). No new source variant, no renderer recursion, no cross-canvas vector-flow-through edge case. The adapter is a higher-level concept (e.g. a `SceneAsVideoSource` wrapper in a later crate) and lives outside the core Scene type.

14. **Scene-sequencing and transitions.** A `Program` is an ordered list of `Scene`s with an optional `Transition` between each adjacent pair. Transitions live _between_ scenes, not inside them. Primitive transitions: `Cut` (no blend), `CrossFade { duration }`, `Wipe { direction, duration }`, `DipToBlack { duration }`. A Program has its own `Axis::Range` computed from the sum of scene durations + transitions; the program renderer drives the current scene, overlaps with the next during a transition window, and composites the two. This is what "image transitions" mean in the common case.

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
    // (No `NestedSequence` — if you need a scene-inside-a-scene,
    // render the inner scene and consume its output as a VideoSource
    // on a leaf SceneObject.  Keeps the Container tree finite.)
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

Justification: PDF needs pages-as-containers; NLE needs tracks-as-containers; compositor uses `Group`/`Layer` as containers for scene graph. The same recursion fits all three. Scene-in-scene is deliberately excluded from the container kinds — the render-then-consume-as-video pattern handles the intro-scene-as-clip case without the complexity of a nested tree.

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
pub enum MediaKind { Image, Video, Audio, Live, Vector, Text }

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
    // No NestedScene variant — see §13. Scene-in-scene is expressed
    // by rendering the inner scene upstream and passing its output
    // here via Source::File (temp-disk) or Source::Generator (in-mem).
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
//
// Every tweakable scalar on this struct — position components, scale,
// opacity, each effect param — is animatable via entries in
// `animations`.  A Keyframe targets a `PropertyPath` on this object
// and the renderer samples the interpolated value at render time.
// So "video source zooms in over 2 seconds" is a `Transform.scale`
// animation on the SceneObject wrapping the VideoSource.  "Picture-
// in-picture fades out" is an `Opacity` animation.  "Rounded corners
// expand" is an animation on the `corner_radius` param of a
// `rounded_mask` Effect.  The Source underneath is untouched.
#[derive(Clone, Debug)]
pub struct SceneObject {
    pub id: ObjectId,
    pub kind: ObjectKind,              // Media(Source) | Text | Shape
    pub transform: Transform,           // position, scale, rotation
    pub range: Range,                   // time-axis lifetime (N/A on Paged)
    pub animations: Vec<Animation>,     // keyframe tracks over any PropertyPath
    pub z_order: i32,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub effects: Vec<Effect>,           // filter chain (colour, blur, sharpen, …)
    pub style: Option<Style>,           // decorative framing (see below)
    pub clip: Option<ClipRect>,
}

/// Decorative framing applied as part of the object's composite —
/// separate from the content-transforming `effects` chain because
/// a rounded-border picture-in-picture is visually paired with the
/// source but isn't a filter over the source pixels. Examples:
/// rounded corners, drop shadow, outer stroke, inner glow.
#[derive(Clone, Debug, Default)]
pub struct Style {
    pub corner_radius: f32,             // px; 0 = no rounding
    pub border: Option<Border>,         // stroke colour + width
    pub drop_shadow: Option<DropShadow>,
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

Canonical JSON for a live compositor scene with one live camera, a pre-rendered intro clip, and a file-based overlay (the intro clip was rendered from a separate scene in a prior pass — here it's just a video file):

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
    "children": [
      { "container": {
          "id": 2,
          "kind": { "layer": { "z_band": [0, 99] } },
          "children": [
            { "leaf": {
                "id": 100,
                "kind": { "media": { "file": { "path": "/assets/intro.mkv", "kind": "video" } } },
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
                "kind": { "media": { "file": { "path": "/assets/lower-third.png", "kind": "image" } } },
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
  "op_log": [
    { "seq": 1, "at": 100, "op": { "add_animation": { "id": 101, "animation": { } } } }
  ]
}
```

Key design points for serde: `Axis` is tagged-enum; `ContainerKind` / `ObjectKind` / `SourceSpec` are tagged-enums; `Source` trait objects serialize via their underlying `SourceSpec` (which is `Clone + Serialize`); `op_log` is optional (only present for compositor replay + NLE undo history). A PDF scene serializes with `axis: { paged: {...} }`, empty `op_log`, and a `Page`-container per page as the roots under `Stage` — nothing else in the format is PDF-specific. A video-embedded-in-PDF is a plain `media.file` child of the owning `Page` container.

## Source kinds legal per axis

PDF / Paged scenes accept only _resolvable-at-author-time_ sources; the exporter has to bake a finished document. Time-based scenes accept everything including live streams.

| `SourceSpec` variant | `Paged` (PDF) | `Range` (NLE) | `Unbounded` (compositor) |
|---|:-:|:-:|:-:|
| `File { kind: Image }`       | ✅ | ✅ | ✅ |
| `File { kind: Video }`       | ✅ (embedded media clip; viewer triggers) | ✅ | ✅ |
| `File { kind: Audio }`       | ✅ (less common in PDF) | ✅ | ✅ |
| `Bytes { … }`                 | ✅ | ✅ | ✅ |
| `DecodedFrame(…)`             | ✅ (pre-rendered bitmap of a page region) | ✅ | ✅ |
| `Live { uri, … }`             | ❌ rejected at validation | ✅ | ✅ |
| `Generator(…)`                | ✅ if deterministic (e.g. noise at frozen seed) | ✅ | ✅ |
| text / shape / vector         | ✅ (object-kind, not Source)  | ✅ | ✅ |

Validation runs at scene-construction time: pushing a `Live` source into a scene whose axis is `Paged` returns `Error::invalid` with a message that points at the axis–source mismatch. The Scene type itself doesn't need a generic parameter for this — the check is a method on `Scene::validate()`.

## Scene ≡ PDF at the data level

The overlap between "a Scene" and "a PDF page" is complete at the data model: both are sets of positioned objects (text runs, vector shapes, images, possibly video clips) on a canvas. The differences are purely:

1. **Axis**: PDF scenes use `Axis::Paged`; video scenes use `Axis::Range` or `Axis::Unbounded`.
2. **Object lifetime**: PDF objects are pinned to a specific `Page` container (their "lifetime" is being a child of that page). Video scene objects use `Range { start, end }` on the time axis.
3. **Source kind restrictions** (see the table above).
4. **Animation semantics**: on a PDF scene the exporter samples animations at a single instant (the page is static); on a time-based scene the renderer samples at each frame.

None of these differences warrant separate types. One `Scene` struct + one `SceneObject` struct + one `Source` enum cover both, and the PDF exporter / video renderer just consult the axis + source kinds to decide what's legal and how to unfold time (or skip it).

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

1. **Embedded-video playback semantics in a PDF.** An `ObjectKind::Media(Source::File { kind: Video })` living on a `Page` container — should the renderer auto-play on page-open, wait for a viewer trigger, or leave the decision to the exporter's PDF annotation builder? Proposal: Scene layer stores the reference + offset; PDF exporter emits a Media Clip annotation with `RunOnOpen = false` by default; viewer-triggered playback is the default.
2. **`Source` trait object or generic?** `Arc<dyn Source>` is serde-unfriendly (we serialize via `SourceSpec`). Are we OK with the asymmetry between "runtime polymorphism via trait object" and "storage/wire via enum"?
3. **`seed: u64` placement.** Should the seed live on `Scene` (global), on each effect (per-node), or both? Proposal assumes scene-global with per-`ObjectId` sub-streams derived via hash.
4. **Paged animation.** Do keyframes in a PDF `Paged` scene mean anything? Given that no object survives between pages, the answer is almost certainly no — animations are per-object and objects live on one page, so the animation has a single page's worth of samples to apply. Proposal: animations on paged-axis scenes are silently ignored by the PDF exporter; keyframes still exist in the data model for consistency with time-based scenes but don't drive vector output.
5. **Transitions as first-class type?** A cross-fade is currently expressed as two overlapping `SceneObject`s with opacity animations. NLE UIs typically want a `Transition` primitive so ripple/roll edits preserve it. Proposal: add `ObjectKind::Transition { outgoing: ObjectId, incoming: ObjectId, kind: TransitionKind, duration: TimeStamp }` later, deferred to v0.2. Program-level transitions (between whole scenes) live on the `Program` type directly and don't need this.
6. **Op-log storage on `Scene`.** Should `op_log` live inside `Scene` (canonical for replay) or outside (passed alongside)? Proposal: optional field `op_log: Option<OpLog>`.
7. **Back-pressure and frame deadlines.** `LiveDrive::frame_deadline` returns a `Duration` — but how does the renderer report missed deadlines upstream? Callback trait? Returned status on `advance_to_wallclock`? Needs interface detail.
8. **Undo granularity.** NLE `OpLog` groups ops via `GroupBegin`/`GroupEnd`. Is a flat grouped-op-log enough, or do we need hierarchical undo (undo the group, then undo a specific op inside)?
9. **`SourceSpec::File` resolution policy.** Is resolution `Source`-trait's job (object-level), or is there a scene-level `Assets` registry that resolves paths once (content-addressed)? Proposal: scene-level registry on `Scene::assets: Assets`.
10. **Scene-as-video-source adapter.** The render-and-consume pattern that replaces nested scenes — should it be a helper in this crate (`SceneAsVideoSource` that wraps a `RasterRender` impl and presents as a `Source`)? Or a separate `oxideav-scene-adapters` crate? Proposal: ship it here as an opt-in module so users don't need a second dep for the common case.
