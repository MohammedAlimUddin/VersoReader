# pdfview — Progress Log

## Milestone 0 — Scaffold, Pdfium, architecture doc
- **Status:** Completed
- **Done:**
  - Created Cargo project with pinned dependencies (`pdfium-render = "=0.9.4"`, `eframe`, `egui`, `crossbeam-channel`, `serde`, `serde_json`, `toml`, `directories`, `blake3`, `lru`) and required release profile.
  - Implemented `scripts/setup_pdfium.sh` to download prebuilt `libpdfium.so` from `bblanchon/pdfium-binaries`.
  - Configured `.gitignore` for `/target/` and `/lib/`.
  - Implemented module skeleton (`pdf`, `ui`, `cache`, `annotations`, `export`, `settings`).
  - Implemented dedicated background worker thread (`pdfview-worker`) with late binding search order:
    1. Directory of the executable
    2. `<exe dir>/lib`
    3. `./lib`
    4. System library
  - Implemented `PdfiumReady` and human-readable `PdfiumError` event notifications.
  - Created dark-themed `eframe` window showing "Pdfium ready" or binding diagnostics.
  - Wrote `ARCHITECTURE.md` detailing thread safety invariants, channels, canonical coordinate system, and module layout.
  - Added automated unit tests for candidate search paths and dynamic binding against `./lib/libpdfium.so`.
- **Known-broken / Deferred:**
  - None for Milestone 0.
- **Manual Verification Checklist:**
  - [x] `bash scripts/setup_pdfium.sh` puts `libpdfium.so` into `lib/`.
  - [x] `cargo run --release` opens a dark window saying "Pdfium ready".
  - [x] Rename `lib/` temporarily: the window displays a readable error without crashing.
  - [x] `ARCHITECTURE.md` exists and accurately describes the system.

---

## Milestone 1 — Open a PDF and render page 1
- **Status:** Completed
- **Done:**
  - Implemented `PathBar` UI component in `src/ui/pathbar.rs`: single text field with an "Open" button, supporting quoted paths (`"..."`, `'...'`), home directory expansion (`~`), and `file://` URI prefix with whitespace trimming.
  - Added inline error messaging for missing files, non-PDF extensions, and Pdfium load errors without UI crashes.
  - Worker thread implements `Command::Open` to streamed-load PDF files via `load_pdf_from_file`, returning total page count and all page sizes in canonical points (origin top-left, unrotated).
  - Worker thread implements `Command::Render` to render requested pages to RGBA bitmaps with configurable width capped at 4096 px.
  - Main UI receives `Event::Rendered`, uploads `egui::ColorImage` into an `egui::TextureHandle`, and displays page 1 fit to the window width in a vertical `ScrollArea`.
  - Added CLI argument support: `pdfview [path/to/file.pdf]` opens the provided file automatically on launch.
  - Added comprehensive automated unit and integration tests:
    - Path parsing validation (empty, missing file, non-PDF, valid fixture, quoted paths, `file://` URIs).
    - Worker fixture tests against `fixtures/MLE.pdf` (open, positive page count, canonical dimensions, non-empty bitmap render with correct RGBA buffer size).
    - Worker fixture test against large book `fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf` verifying document opening and page count.
- **Known-broken / Deferred:**
  - Continuous vertical multi-page scrolling and caching are scheduled for Milestone 2.
- **Manual Verification Checklist:**
  - [x] Paste a full path of your large book: page 1 appears quickly and looks identical in quality to Edge.
  - [x] Paste the path with quotes and with `~`: works.
  - [x] Paste a wrong path: inline error, no crash.
  - [x] The UI stays responsive while the large book opens.
  - [x] Launch with CLI argument: `cargo run --release -- "fixtures/MLE.pdf"` opens the document immediately on start.

---

## Milestone 2 — Continuous scrolling, lazy rendering, cache
- **Status:** Completed
- **Done:**
  - Implemented `src/cache.rs`: LRU texture cache with a strict **150 MB** hard memory cap using `lru::LruCache`, evicting and dropping `egui::TextureHandle` resources on overflow.
  - Implemented virtualized continuous page layout in `src/ui/viewer.rs`: calculates page bounds from canonical page dimensions, centers pages horizontally, and only renders/paints pages intersecting the viewport (+1 prefetch on each side).
  - Implemented progressive rendering: requests low-resolution (~35%) render first for rapid visual response, swapped seamlessly with full-resolution render when complete.
  - Added light-gray placeholder page rectangles with crisp borders while pages are loading.
  - Implemented generation counter for `Render` commands: worker drops stale render commands and UI ignores stale render results.
  - Implemented `Toolbar` in `src/ui/toolbar.rs`: editable page box ("N of M") that continuously updates with scroll position and jumps to target page on Enter.
  - Implemented keyboard scrolling: Arrow Up/Down, Page Up/Down, Space, Home, and End.
  - Preserved event-driven repainting invariant (~0% idle CPU; repaints only on worker events or user inputs).
  - Implemented F3 debug overlay: displays live cache memory usage (MB / 150 MB), cached page texture count, frame time, and FPS.
- **Known-broken / Deferred:**
  - Zoom and rotation view projections are scheduled for Milestone 3.
- **Manual Verification Checklist:**
  - [ ] Open the 300+ page book, drag the scrollbar rapidly to the middle and the end: pages fill in quickly, no freeze.
  - [ ] Press `F3`: cache size never exceeds ~150 MB after a long scroll.
  - [ ] `ps -o rss= -p $(pgrep pdfview)` stays reasonable (target under ~200 MB at 100% zoom).
  - [ ] Idle: `top` shows ~0% CPU when you aren't scrolling.
  - [ ] Page box updates while scrolling; typing a number and Enter jumps there.
  - [ ] Keyboard navigation: Arrow keys, PgUp, PgDn, Space, Home, and End scroll smoothly.
