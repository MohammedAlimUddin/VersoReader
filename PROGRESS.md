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
  - [x] Open the 300+ page book, drag the scrollbar rapidly to the middle and the end: pages fill in quickly, no freeze.
  - [x] Press `F3`: cache size never exceeds ~150 MB after a long scroll.
  - [x] `ps -o rss= -p $(pgrep pdfview)` stays reasonable (target under ~200 MB at 100% zoom).
  - [x] Idle: `top` shows ~0% CPU when you aren't scrolling.
  - [x] Page box updates while scrolling; typing a number and Enter jumps there.
  - [x] Keyboard navigation: Arrow keys, PgUp, PgDn, Space, Home, and End scroll smoothly.

---

## Milestone 3 — Zoom, rotate, fullscreen
- **Status:** Completed
- **Done:**
  - Implemented `src/pdf/transform.rs`: `PageTransform` helper mapping between canonical space (points, origin top-left, unrotated) and screen coordinates with zoom, rotation (0°, 90°, 180°, 270°), and screen origin.
  - Added automated unit tests for `PageTransform` verifying exact coordinate roundtrips across all four rotations.
  - Integrated zoom controls in toolbar: `-` / `+` buttons, percentage display, "Fit Width", and "Fit Page". Clamped to 10%–800%.
  - Added zoom shortcuts: `Ctrl + =` / `Ctrl + +` (zoom in), `Ctrl + -` (zoom out), `Ctrl + 0` (fit width), and `Ctrl + scroll` (smooth zoom around cursor position).
  - Anchor zoom mathematics: maintains content position fixed under cursor (or viewport center for toolbar buttons).
  - Texture size capped at 4096 px per side.
  - Implemented 90° clockwise rotation button (`⟳ Rotate`) that updates all pages, preserves active page scroll position, and delegates rotation to Pdfium via `PdfRenderConfig::rotate()`.
  - Implemented Fullscreen mode: toggle button and `F11` key; hides all window chrome (path bar, toolbar, debug bar); `Esc` or `F11` exits.
  - Updated worker thread to accept and apply rotation settings, caching textures by `(doc_id, page, width_px, rotation)`.
  - **Production-Grade Sharpness & Edge Parity:**
    - Worker thread uses `PdfBitmapFormat::BGRA` with `use_lcd_text_rendering(true)`, `set_text_smoothing(true)`, `set_image_smoothing(true)`, and `set_path_smoothing(true)`.
    - **Removed `use_print_quality(true)`**: In PDFium, `use_print_quality` passes the `FPDF_PRINTING` flag, which switches FreeType to printer rasterization and explicitly disables on-screen font hinting and LCD subpixel antialiasing. Removing it restores full on-screen FreeType font hinting and LCD subpixel rendering.
    - **Eliminated GPU Mipmap Blurring**: Set `mipmap_mode: None` in `TextureOptions`. In OpenGL, trilinear filtering (`GL_LINEAR_MIPMAP_LINEAR`) blends with downsampled mipmap levels when subpixel floating-point offsets occur during scrolling, creating a soft haze on 1:1 text. Disabling mipmaps ensures razor-sharp 1:1 texel rendering.
    - **2D Bit-Perfect Device Pixel Alignment**: Aligned both width and height of `screen_rect` to the exact physical device pixel dimensions of the PDFium bitmap (`exact_w_pts = sharp_width / ppp; exact_h_pts = bitmap_h_px / ppp; origin = snap(raw_origin)`), eliminating periodic vertical interpolation / moiré blur across horizontal font stems.
    - Standard 96 DPI scale (`PageTransform::DPI_SCALE = 96.0 / 72.0`) matches Microsoft Edge's default display metrics.
  - **Native Touchpad Pinch Zoom (Wayland `zwp_pointer_gestures_v1`):**
    - Implemented dedicated Wayland gesture listener in `src/platform/wayland.rs` running on background thread `pdfview-wayland-gestures`.
    - Binds compositor protocol `zwp_pointer_gestures_v1` and `wl_seat::get_pointer()` from the foreign Wayland display (`RawDisplayHandle::Wayland`).
    - Receives multi-finger trackpad pinch `Begin`, `Update(scale)`, and `End` events from GNOME Mutter without requiring `Ctrl` key.
    - Streams relative continuous magnification factors (`current_scale / last_scale`) over `crossbeam-channel` to UI loop, scaling seamlessly around cursor hover coordinates at 60 fps.
    - Preserved `Ctrl + two-finger scroll` and touchscreen multi-touch pinch anchoring as complementary inputs.
  - **Edge-Style Arrow Navigation & 2D Scrolling:**
    - `ArrowLeft`: jumps to previous page.
    - `ArrowRight`: jumps to next page.
    - `ArrowUp` / `ArrowDown`: scrolls vertically by 80 px.
    - Integrated `ScrollArea::both()` with dynamic content width sizing, allowing horizontal panning when zoomed in beyond window width.
- **Known-broken / Deferred:**
  - Tabs are scheduled for Milestone 4.
- **Manual Verification Checklist:**
  - [x] Zoom with buttons, `Ctrl + scroll` on touchpad/wheel, `Ctrl + =`, `Ctrl + -`, and `Ctrl + 0`; text is razor-sharp and matches production-grade Edge quality.
  - [x] Touchpad two-finger pinch zoom without `Ctrl` zooms smoothly and continuously around cursor on Ubuntu Wayland.
  - [x] Zooming heavily (up to 800%) stays rock-solid, stable, does not loop or flicker, and memory stays capped under 150 MB.
  - [x] Touchpad `Ctrl + two-finger scroll` scales smoothly and continuously around the pointer position.
  - [x] ArrowLeft / ArrowRight navigate between pages; ArrowUp / ArrowDown scroll vertically.
  - [x] When zoomed in beyond window width, horizontal scrollbars appear and horizontal scrolling/panning is enabled.
  - [x] Rotate 4 times returns to the original orientation; scroll position remains on the current page.
  - [x] `F11` toggles fullscreen and hides chrome; `Esc` exits fullscreen.
