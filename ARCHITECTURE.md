# pdfview Architecture

## 1. Overview
`pdfview` is a minimalistic, fast, low-RAM desktop PDF reader for Linux written in Rust, utilizing `pdfium-render` dynamically bound to Google Pdfium (`libpdfium.so`) and `eframe`/`egui` with the `glow` backend.

---

## 2. Thread Model & Channel Protocol

### Single Worker Invariant
Pdfium is strictly single-threaded. Per design rule:
* **The UI thread never calls Pdfium** and never blocks (no disk I/O, no channel blocking, no sleeps).
* **Pdfium runs exclusively on one worker thread** (`pdfview-worker`).
* A single `Pdfium` instance is initialized on the worker thread and leaked (`Box::leak`) to provide a `'static` reference, enabling `PdfDocument<'static>` handles to live in a `HashMap<DocId, PdfDocument<'static>>`.

```
+-------------------------------------------------------------+
|                          UI Thread                          |
|  - eframe / egui event loop                                 |
|  - Virtualized layout, texture cache (LRU 150 MB cap)       |
|  - Canonical <-> Screen coordinate projection (PageTransform) |
+-------------------------------------------------------------+
         |                                           ^
         | Command (unbounded channel)               | Event (unbounded channel)
         v                                           | + ctx.request_repaint()
+-------------------------------------------------------------+
|                        Worker Thread                        |
|  - Single owner of Pdfium instance and PdfDocument handles   |
|  - Late-bound libpdfium.so                                  |
|  - Serves Render, Outline, Search, PageText, Export jobs    |
|  - Page-by-page chunking with cancellation & priority       |
+-------------------------------------------------------------+
```

### Channel Protocol
* **UI → Worker (`Command`)**:
  * `Open { doc_id, path }`: Stream-loads PDF via `load_pdf_from_file` and returns page count and canonical page dimensions.
  * `Close { doc_id }`: Drops document handle.
  * `Render { doc_id, page, width_px, rotation, generation, priority }`: Renders page bitmap.
  * `Outline { doc_id }`: Fetches bookmark/outline hierarchy.
  * `PageText { doc_id, page }`: Extracts character bounding boxes in canonical coordinates.
  * `Search { doc_id, query_id, query }`: Chunked per-page text search.
  * `SyncAnnotations { doc_id, highlights }`: Synchronizes in-memory `/Highlight` and `/Ink` annotations onto worker's document handle.
  * `SaveDoc { doc_id, path, highlights }`: Embeds ISO 32000 `/Highlight` & `/Ink` annotations and atomically saves PDF via temporary file + rename.
  * `Export { ... }`: Exports annotated PDF.
  * `Shutdown`: Exits worker loop cleanly.

* **Worker → UI (`Event`)**:
  * `PdfiumReady` / `PdfiumError(String)`
  * `Opened { doc_id, page_count, page_sizes, annotations }`
  * `Rendered { doc_id, page, width_px, rotation, generation, image }`
  * `OutlineResult { doc_id, items }`
  * `PageTextResult { doc_id, page, chars, lines }`
  * `SearchResults { doc_id, query_id, page, matches, done }`
  * `Saved { doc_id, success, error }`
  * `ExportResult { ... }`

* **Repaint Notification**: After sending any event, the worker invokes `ctx.request_repaint()` to wake the UI thread.
* **Generation Counter**: Every `Render` request carries an incrementing `generation` ID bumped whenever zoom, scroll, or rotation changes. The worker drops stale generation requests, and the UI discards late-arriving stale results.

---

## 3. Dynamic Library Binding Search Order

At startup, the worker thread attempts to load `libpdfium.so` dynamically in the following strict order:
1. Directory containing the executable (`<exe_dir>/libpdfium.so`)
2. `<exe_dir>/lib/libpdfium.so`
3. `./lib/libpdfium.so` (project-local directory)
4. Operating system shared library paths (`Pdfium::bind_to_system_library()`)

If all fail, the worker sends `Event::PdfiumError` with the exact searched paths and a guide to run `scripts/setup_pdfium.sh`.

---

## 4. Coordinate System & Transformations

### Canonical Coordinate Space
* **Definition**: Points ($1/72$ inch), origin at **top-left**, unrotated page ($0^\circ$).
* **Conversion Boundary**: Pdfium's native coordinate space has origin at the bottom-left. Coordinates are converted **once**, at the worker boundary, into canonical space.
* **Storage Invariant**: All stored geometries (search results, character bounding boxes, highlights, text boxes) are stored strictly in canonical space.

### View Projection (`PageTransform`)
* Zoom and rotation ($0^\circ, 90^\circ, 180^\circ, 270^\circ$) are purely view-time projections.
* `PageTransform` maps `canonical_point <-> screen_point` using zoom, rotation, page size, and viewport offset.
* Every draw call and hit-test passes through `PageTransform`.

---

## 5. Rendering Pipeline & Texture Cache
* **Virtualization**: Layout is calculated upfront using page sizes returned at open. Only pages intersecting the viewport plus 1 page of prefetch above/below are rendered and drawn.
* **Sharp Rendering & GPU Fallbacks**: Visible pages are requested at exact device pixel resolution (`sharp_width`) with top priority (`priority: 1`), rendering in ~30ms directly from PDFium without initial low-res blurriness. During active zooming or annotation updates, existing cached textures serve as GPU-filtered bilinear fallbacks until the new sharp texture finishes rendering, preventing white flashes or blur lockups.
* **Worker Command Scheduling**: The background worker thread prunes stale generation requests upon receiving new zoom/scroll commands and prioritizes visible page renders before prefetch tasks.
* **Texture Limits**: Maximum texture side is capped at 6144 px.
* **LRU Cache (`cache.rs`)**: Keyed by `(doc_id, page, width_px, rotation)` with a hard **150 MB** memory cap and generation tracking (`CachedTexture.generation`). When an annotation is added/erased or zoom changes, generation is bumped and the previous texture is painted as a seamless fallback while PDFium generates the new texture with native `FXDIB_BLEND_MULTIPLY` blending. Switching tabs drops the inactive tab's textures from the cache while retaining the worker's document handle; closing a tab sends `Command::Close` and immediately purges all associated textures.

---

## 6. Module Structure

```
src/
  main.rs          eframe application entry point and CLI arg parsing
  app.rs           Main application state, event drain, top-level update loop
  cache.rs         LRU texture cache (150 MB cap) and LRU PageTextCache (32 pages cap)
  annotations.rs   Highlight and TextBox data structures, hit-testing, sidecar JSON
  export.rs        PDF annotation export writer
  settings.rs      Settings persistence (settings.toml)
  pdf/
    mod.rs         PDF module exports
    worker.rs      Dedicated background Pdfium worker thread
    types.rs       Command and Event definitions
    transform.rs   Canonical <-> screen coordinate transforms (PageTransform)
  platform/
    mod.rs         Platform-specific abstractions
    wayland.rs     Native Wayland touchpad pinch gesture listener (zwp_pointer_gestures_v1)
  ui/
    mod.rs         UI components export
    tabbar.rs      Document tabs and Settings tab navigation
    pathbar.rs     Collapsible path input and open bar
    toolbar.rs     Page controls, zoom, rotation, tools, settings
    sidebar_toc.rs Table of contents outline tree
    findbar.rs     In-document search bar
    viewer.rs      Continuous virtualized page viewer
    settings_tab.rs Application settings view
scripts/
  setup_pdfium.sh  Automated downloader for prebuilt libpdfium.so
ARCHITECTURE.md    System architecture documentation
PROGRESS.md        Milestone checklist and verification status
```

---

## 7. Annotation Storage & Lifecycle
* **Storage Invariant**: Annotations (`Highlight`, `TextBox`) are stored strictly in canonical coordinates (points, unrotated, origin top-left).
* **Storage Path**: Persisted as JSON in `~/.local/share/pdfview/annotations/<blake3(abs path + file size)>.json`.
* **Direct PDF Embedding (`Ctrl+S`)**: Native ISO 32000 `/Highlight`, `/Ink`, and `/FreeText` annotations are embedded directly into the document using an atomic temporary-file-and-rename pipeline, enabling seamless viewing across Microsoft Edge, Chrome, and Adobe Acrobat.
* **Document Persistence & Lifecycle**:
  - When opening a document, annotations are extracted directly from the PDF file (`extract_annotations_from_doc`).
  - Unsaved annotations are discarded on tab close or exit without overwriting the user's PDF, ensuring pristine clean reopen states.
  - Pressing `Ctrl+S` or clicking Save embeds all annotations into the PDF file.
* **Undo / Redo**: Bounded per-tab undo/redo stack (`UndoStack`, max 50 actions) for highlights and text boxes (`Add`, `Delete`, `Modify`), bound to `Ctrl+Z` and `Ctrl+Y` / `Ctrl+Shift+Z`.
* **Hit-Testing & Interaction Tools**:
  - **Highlighter**: Dragging over text creates line-height-adjusted highlight rectangles (`adjust_rect_thickness`), centering the band vertically on lines. Dragging in whitespace or margins creates freehand ink strokes. Drawn with native PDFium `FXDIB_BLEND_MULTIPLY` blending.
  - **Text Tool**: Clicking on a page creates an inline `egui::TextEdit::multiline` text box. Double-clicking existing boxes edits them; clicking and dragging moves them across pages with full rotation support; pressing `Delete` deletes selected boxes; empty boxes are discarded on commit. Text is rendered continuously using egui's text layout engine over a transparent background matching the selected font color.
  - **Text Box Resizing**: Selected and actively editing text boxes feature an interactive circular handle on the right edge. Hovering displays `CursorIcon::ResizeHorizontal`; dragging resizes box width (clamped $\ge 40\text{ pt}$) with dynamic multiline text reflow and wrapping; mouse release pushes an undoable `ModifyTextBox` action.
  - **Eraser**: Sets cursor to `Crosshair`. Clicking or dragging across highlights or text boxes hit-tests points in canonical space and deletes them, pushing deletions to the undo stack.
  - **Settings**: Active highlighter color/thickness, text font size, text color, and touchpad/mouse wheel multipliers persist in `~/.config/pdfview/settings.toml`.

