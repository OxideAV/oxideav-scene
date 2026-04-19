# Use case: RTMP streaming compositor

A long-running daemon that maintains one `Scene` per live output
channel. External clients — an OBS-style dashboard, a game server, a
producer's remote — push incremental `Operation`s over a control
channel (WebSocket JSON, gRPC, …). The daemon renders the scene into
a video/audio encoder pair and pushes the result to an RTMP ingest.

## Data flow

```
                ┌─────────────────────────────────────┐
                │  Control plane (WebSocket / gRPC)   │
                │    AddObject / Animate / …          │
                └─────────────────────────────────────┘
                                │
                                ▼
  ┌─────────────┐        ┌─────────────┐        ┌─────────────┐
  │ Live input  │──────▶ │   Scene     │ ─────▶│ SceneRenderer│
  │ (RTMP in,   │        │ (indefinite)│        │              │
  │  camera, …) │        └─────────────┘        └─────────────┘
  └─────────────┘                                       │
                                                        ▼
                                            ┌─────────────────┐
                                            │ Encoder + RTMP   │
                                            │ muxer (FLV out)  │
                                            └─────────────────┘
```

`Scene::duration` is `SceneDuration::Indefinite`. The render loop
drives time forward from wall-clock, pulling `Operation`s from the
control-plane queue between ticks.

## Operation DSL

The full set lives in [`ops::Operation`](../src/ops.rs). A minimal
JSON wire format could look like:

```json
{
  "op": "add_object",
  "id": 42,
  "kind": { "image": { "path": "/assets/lower_third.png" } },
  "transform": { "position": [0, -100], "anchor": [0, 0] },
  "lifetime": { "start": "now" },
  "z_order": 10
}

{
  "op": "animate",
  "id": 42,
  "property": "position",
  "keyframes": [
    { "t_ms_from_now": 0,    "value": { "vec2": [0, -100] } },
    { "t_ms_from_now": 800,  "value": { "vec2": [0, 0] },   "easing": "ease_out" }
  ]
}

{ "op": "remove_object", "id": 42, "at_ms_from_now": 5000 }
```

Clients specify timing relative to "now"; the server resolves these
into absolute scene timestamps before inserting into the
`Animation` / `AudioCue` tracks.

## Latency considerations

The compositor renders at the output framerate (typically 30 or 60
FPS). For a 30 FPS target each tick has ~33 ms to:

1. Drain pending operations.
2. Advance all `Video` / `Live` sources.
3. Sample every animation at the current `t`.
4. Composite the scene.
5. Encode the frame + hand to the RTMP muxer.

Hot paths that need real attention once the rendering crate lands:

- **Bitmap compositing** — the main bottleneck. SIMD-ise via the
  same chunked `std::simd` pattern used in `oxideav-vorbis` +
  `oxideav-pixfmt`.
- **Sub-pel MC for `Live` sources** — reuse oxideav's existing
  per-codec MC kernels (VP8/VP9/H.264/H.265/AV1 all have 8-tap
  filters in-tree).
- **Text shaping** — cache glyph bitmaps per (font, size, character)
  so animated text doesn't re-shape every frame.

## Determinism and failure modes

- Control-plane operations are applied in the order received.
  Clients that need causal ordering should include a `seq` field;
  the server serialises by it.
- If an operation would make the scene invalid (missing `ObjectId`,
  animation on a nonexistent property), the server replies with an
  error on the control channel and leaves the scene unchanged.
- A `Live` source going dead (RTMP input disconnects) holds its
  last frame for `hold_ms` (config knob), then the object switches
  to transparent until the source reconnects.
