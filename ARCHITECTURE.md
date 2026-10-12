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
  * `Export { ... }`: Exports annotated PDF.
  * `Shutdown`: Exits worker loop cleanly.

* **Worker → UI (`Event`)**:
  * `PdfiumReady` / `PdfiumError(String)`
  * `Opened { doc_id, page_count, page_sizes }`
  * `Rendered { doc_id, page, width_px, rotation, generation, image }`
  * `OutlineResult { doc_id, items }`
  * `PageTextResult { doc_id, page, chars, lines }`
  * `SearchResults { doc_id, query_id, page, matches, done }`
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
* **Progressive Rendering**: Low-resolution (~35% scale) render requested first for responsive feedback, swapped with full-resolution render once ready.
* **Texture Limits**: Maximum texture side is capped at 4096 px.
* **LRU Cache (`cache.rs`)**: Keyed by `(doc_id, page, width_px, rotation)` with a hard **150 MB** memory cap, actively freeing textures upon eviction or tab closure.

---

## 6. Module Structure

```
src/
  main.rs          eframe application entry point and CLI arg parsing
  app.rs           Main application state, event drain, top-level update loop
  cache.rs         LRU texture cache with byte size cap
  annotations.rs   Highlight and TextBox data structures, hit-testing, sidecar JSON
  export.rs        PDF annotation export writer
  settings.rs      Settings persistence (settings.toml)
  pdf/
    mod.rs         PDF module exports
    worker.rs      Dedicated background Pdfium worker thread
    types.rs       Command and Event definitions
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

