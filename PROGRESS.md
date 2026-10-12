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
  - [ ] Paste a full path of your large book: page 1 appears quickly and looks identical in quality to Edge.
  - [ ] Paste the path with quotes and with `~`: works.
  - [ ] Paste a wrong path: inline error, no crash.
  - [ ] The UI stays responsive while the large book opens.
  - [ ] Launch with CLI argument: `cargo run --release -- "fixtures/MLE.pdf"` opens the document immediately on start.
