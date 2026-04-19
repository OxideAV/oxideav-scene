# Use case: PDF pages

The PDF integration treats each page as a scene. Page objects (glyph
runs, images, vector paths) become `SceneObject`s; the page's media
box becomes the `Canvas::Vector`; edits happen on the scene graph
without ever rasterising.

## Producing a Scene from a PDF page

A future `oxideav-pdf` crate will parse the page content stream
(PDF's "operators"):

| PDF operator | SceneObject produced |
|---|---|
| `Tj` / `TJ` (show text) | `ObjectKind::Text` with `font_family` + `advances` pinned from the actual glyph metrics |
| `Do /XObject` (image) | `ObjectKind::Image(ImageSource::Decoded(...))` |
| `f` / `S` / `B` (fill/stroke) | `ObjectKind::Shape(Shape::Path { data })` |
| `cm` (transformation matrix) | stacked into the containing group's `Transform` |
| `q` / `Q` (save/restore graphics state) | begin/end a `Group` |

The scene's `time_base` is arbitrary (we use `1/1000`) and its
duration is `Finite(1)` — a PDF page is one frame.

## Edits

Edits are just operations on `scene.objects`:

- Redaction → find objects overlapping a `ClipRect`, set their
  opacity to 0 or delete them.
- Watermark → append an `ObjectKind::Image` with a high `z_order` +
  transparent `opacity: 0.2`.
- Text replacement → find `ObjectKind::Text` by string match, swap
  its `text` field, optionally re-shape.
- Paragraph reflow → group successive text runs, reflow into the
  container's new `ClipRect`.

Because no rasterisation happens until export, text remains
selectable through the round-trip.

## Exporting back to PDF

The PDF exporter is a `SceneRenderer` impl that walks the tree in
paint order and emits PDF content-stream bytes via
`ExportOp::Raw { format: "pdf", payload }`:

- `Text` → `BT / Tf / Tj / ET` sequence, advances preserved
- `Image` → reserve XObject, emit `Do /XObj0`
- `Shape::Path` → `data` is already SVG-ish; convert to PDF path
  operators (`m`, `l`, `c`, `f`, `S`)
- `Transform` → flatten into a `cm` matrix

Bookmarks, hyperlinks, form fields, and annotations ride in
`metadata` + a dedicated `Annotation` object kind (to be added when
the PDF crate lands).

## Non-goal today

Full PDF semantics — encrypted PDFs, embedded fonts (OTF/TTF/CFF
parsing lives in `oxideav-text`, pending), colour profiles, JBIG2
images, and the scripting engine. Basic text + image + vector pages
come first.
