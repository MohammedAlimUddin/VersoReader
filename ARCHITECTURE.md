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
    ipc.rs         Single-instance Unix domain socket IPC listener and client
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

## 7. Single-Instance IPC Architecture (`src/platform/ipc.rs`)
* **Socket Path**: Unix domain socket at `$XDG_RUNTIME_DIR/verso-reader.sock` (fallback: `/tmp/verso-reader-<uid>.sock`).
* **CLI Interception**: On launch, `main.rs` inspects whether an IPC socket is actively listening:
  - If a file path is provided: sends `OPEN <canonical_path>`.
  - If no path is provided: sends `FOCUS`.
  - If the socket responds with `OK`, the secondary process exits immediately (`0`), handing off the request to the active instance.
  - If `ConnectionRefused` occurs, the stale socket file is unlinked automatically, allowing the current process to claim server ownership.
* **Server Lifecycle**:
  - The background IPC thread runs an accept loop on the socket, forwarding incoming `IpcMessage::Open(PathBuf)` and `IpcMessage::Focus` commands to the UI thread over a crossbeam channel.
  - On shutdown, `IpcServer::drop` automatically unlinks the socket file.
* **Window Activation & Tab Management**:
  - When an `Open` message arrives, the UI checks if the document is already open in an existing tab. If so, it switches focus to that tab; otherwise, it opens the file in a new tab (or reuses the default empty tab).
  - The application requests OS window focus using `egui::ViewportCommand::Focus`, `egui::ViewportCommand::Minimized(false)`, and `egui::ViewportCommand::RequestUserAttention(Critical)`.

---

## 8. In-PDF Hyperlinks & URL Actions
* **Link Extraction (`pdf/worker.rs`)**:
  - Worker queries both `page.links()` and `page.annotations()` during text extraction.
  - Resolves internal destinations (`XYZ`, `Fit`, page index) and external URI actions (`http`, `https`, `mailto`).
  - Converts link bounding boxes into canonical coordinate space (`PageLink { rect, target }`) and caches them inside `PageTextData`.
* **Interaction & Tooltips (`ui/viewer.rs`)**:
  - When the pointer hovers over a link, the cursor transforms into `CursorIcon::PointingHand`.
  - Tooltips display destination details: `"Go to page N"` for internal document jumps, or the full URL for external web links.
  - **Click vs. Drag Differentiation**: A primary mouse click executes the link action (`ViewerAction::JumpToPage` or `ViewerAction::OpenUrl`), while mouse drag gestures seamlessly transition into text selection without activating the link.
  - Internal page jumps are recorded in the tab's navigation history, allowing instant backward and forward traversal via `Alt+Left` / `Alt+Right`. External URLs open in the default web browser via `ctx.open_url`.

---

## 9. Annotation Storage & Lifecycle
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
  - **Settings**: Active highlighter color/thickness, text font size, text color, and touchpad/mouse wheel multipliers persist in `~/.config/pdfview/settings.toml`.

---

## 10. External Annotation Rendering & Solid Block Elimination
* **Problem Solved**: External PDF editors (Adobe Acrobat, Microsoft Edge, Xournal++) write `/FreeText`, `/Text` (sticky notes), and `/Popup` annotations with appearance streams or default background fills. When `render_annotations(true)` is enabled in PDFium, PDFium burns opaque rectangular black or yellow boxes into the page bitmap, while egui simultaneously draws the text on top.
* **Extraction & Clean In-Memory State**:
  - `extract_annotations_from_page` in `src/pdf/worker.rs` extracts all text content, geometry, font size, and color into canonical `TextBox` models.
  - Immediately following extraction, `/FreeText`, `/Text`, and associated `/Popup` annotations are deleted from the in-memory `PdfPage` (`annots.delete_annotation(annot)`).
  - In `Command::Render`, a safety check purges any remaining solid-block annotations before `page.render_with_config`, guaranteeing that visible pages never render with opaque backgrounds regardless of prefetch or race timing.
* **Transparent egui Text Layering**:
  - Text annotations are rendered exclusively through egui's galley layout engine with transparent backgrounds and the exact author-selected font colors.
  - Native save (`Command::SaveDoc`) serializes standard ISO 32000 `/FreeText` annotations with transparent interior color (`/IC`) and borderless style, and cleans the in-memory document immediately after saving.

---

## 11. Dormant Tab Memory Optimization
* **Inactivity Tracking**:
  - Each `PdfTab` tracks `last_accessed: Instant` and `is_dormant: bool`.
  - The UI check loop (`check_dormant_tabs`) monitors background tabs. Any tab inactive for $> 5\text{ minutes}$ ($300\text{ seconds}$) is transitioned to dormant state.
* **Three-Tier Resource Deallocation**:
  1. **OpenGL Texture Cache**: Purges all GPU texture handles (`cache.clear_doc(doc_id)`).
  2. **Page Text Cache**: Drops extracted character bounding boxes (`text_cache.clear_doc(doc_id)`).
  3. **Worker Pdfium Document**: Dispatches `Command::UnloadDoc { doc_id }`, which drops the `PdfDocument` handle and releases C++ font tables and page buffers.
* **Minimal Memory Footprint & Seamless Wakeup**:
  - Dormant tabs retain only ~1 KB of tab metadata in RAM (path, page count, scroll offset, zoom, rotation, annotations, nav history).
  - In the tab bar, dormant tabs display italicized titles and a sleeping tooltip: `"Sleeping tab to save memory — click to wake"`.
  - Clicking a dormant tab awakens it instantly: `ensure_active_tab_awake` reloads the PDF on the worker thread ($< 30\text{ ms}$) and synchronizes any unsaved annotations without viewport jump or flickering.

