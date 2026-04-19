# Use case: NLE timeline (Premiere / Resolve style)

Scenes map cleanly onto the editing model used by pro NLE software.

## Tracks as groups

A track is an `ObjectKind::Group` whose children are `SceneObject`s
placed sequentially on the timeline, each with its own `Lifetime`.
Multiple tracks live in the same scene at different `z_order`
bands — e.g. track V1 at `z_order = 0`, V2 at `10`, V3 at `20`.
The scene is the sequence's master.

```
z=20  V3    │                        ┌──────┐
z=10  V2    │       ┌────────────────┤      ├──────
z= 0  V1    │───────┤                └──────┘
            └──── timeline ────────────────────────
```

## Clips as SceneObjects

A "clip" in NLE terms is:

- `ObjectKind::Video(VideoSource::Path(...))` for a video clip,
  `Image(...)` for a still, `Text(...)` for a title card.
- `Lifetime { start, end }` matching the in/out points on the
  timeline.
- `Transform` — position/scale/rotation set by the user.
- `animations` — any keyframe track the user has placed (position
  moves, opacity fades, scale for "Ken Burns" effect, …).
- `effects` — colour correction, chroma key, blur, …

## Transitions

A cross-fade is just two overlapping `SceneObject`s with an `Opacity`
animation on each:

- Outgoing clip: opacity `1.0 → 0.0` over the transition duration.
- Incoming clip: opacity `0.0 → 1.0` over the same interval.

Wipes use a custom `effects` entry that masks the frame in a moving
shape.

## Audio tracks

Audio clips are `AudioCue`s, not `SceneObject`s — positional
compositing doesn't apply. A stereo cue's L/R imbalance is carried
through the PCM source; multi-track mixing happens in the renderer's
bus stage.

## Export

The exporter is a `SceneRenderer` implementation that:

1. Walks timestamps from `0` to `scene.duration.end().unwrap()` at
   the project framerate.
2. At each tick, collects `visible_at(t)`, samples each animation,
   composites into a frame.
3. Feeds the frame into an encoder (H.264 / H.265 / ProRes /
   whatever the user chose) via `oxideav-codec`.
4. Mixes the active `AudioCue`s into the output bus, feeds through
   the audio encoder.
5. Muxes into the output container.

## Scrubbing + preview

The editor UI calls `render_at(t)` for arbitrary `t` as the user
drags the playhead. The renderer is free to cache per-object decoder
state across adjacent timestamps; the `SceneSampler` trait is
designed so that each object's "natural" decode-forward path is
preserved within a render session.

## Non-goal today

Edit decision lists (EDL / AAF / OMF), proxy workflows, per-track
effects, multicam, keyframable masks. These are straightforward
extensions once the core render pipeline is real.
