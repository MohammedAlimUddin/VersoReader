# pdfview — project brief for coding agents

You are building **pdfview**, a minimalistic, very fast, low-RAM desktop PDF reader for Ubuntu, written in Rust. It imitates the PDF reading experience of Microsoft Edge, minus everything the owner does not need. The owner oversees; you write the code. Read this whole file at the start of every session, then read `PROGRESS.md`.

## Hard rules (never break)

1. **One milestone at a time.** Work only on the milestone the owner names. Do not start the next one, and do not add features that are not in this spec.
2. **Minimal diffs.** Edit existing files surgically. Never rewrite a whole file unless the milestone requires it. Never reformat unrelated code.
3. **Pdfium runs on exactly one thread** (the worker thread). The UI thread must never call pdfium and must never block (no sleeps, no waiting on channels, no reading large files from disk).
4. **Never invent APIs.** Before using any `pdfium-render`, `egui` or `eframe` item, check its docs for the *pinned version* (docs.rs, `cargo doc`, or the crate source in `~/.cargo/registry`). If unsure, say so and look it up. Do not rely on memory; APIs changed between versions.
5. **No new dependencies** beyond the allowed list below without asking the owner first.
6. **No `unsafe`** unless there is no alternative; if used, isolate it and comment why.
7. **Never overwrite the user's PDF files.** Annotations live in our own data; exporting writes a new file.
8. If a requirement is ambiguous or a milestone cannot be done as written, stop and ask instead of guessing.

## Stack (fixed)

| Part | Choice |
|---|---|
| Language | Rust (stable, edition 2021 or later) |
| PDF engine | `pdfium-render` (pin one exact version in `Cargo.toml`, e.g. `=0.9.x`, after checking the latest on docs.rs), dynamically bound to `libpdfium.so` |
| UI | `eframe` + `egui` with the `glow` backend (no wgpu), X11 and Wayland features on. Verify feature names in the eframe docs |
| Threads | `std::thread` + `crossbeam-channel` |
| Persistence | `serde`, `serde_json` (annotations), `toml` (settings), `directories` (paths), `blake3` (file keys) |

Allowed dependencies: `eframe`, `egui`, `pdfium-render`, `crossbeam-channel`, `serde`, `serde_json`, `toml`, `directories`, `blake3`, `lru` (or a hand-written LRU). Nothing else without asking.

`Cargo.toml` release profile (required):

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

## Pdfium setup

- `scripts/setup_pdfium.sh` downloads the Linux x64 prebuilt library from the `bblanchon/pdfium-binaries` GitHub releases (`pdfium-linux-x64.tgz`) and places `libpdfium.so` in `./lib/`. `lib/` is git-ignored.
- At startup, bind in this order: directory of the executable, `<exe dir>/lib`, `./lib`, then the system library. Show a clear error screen (not a panic) if none is found.
- Create the `Pdfium` instance once on the worker thread and leak it (`Box::leak`) so documents can be stored in a map with `'static` lifetime.
- Open files with `load_pdf_from_file` (streamed from disk). Do not load whole PDFs into RAM with `load_pdf_from_byte_vec`.

## Architecture

```
src/
  main.rs            eframe entry, optional CLI arg (file path to open on start)
  app.rs             App state, top-level update loop, Tab enum (Pdf | Settings)
  cache.rs           LRU texture cache with a byte cap
  annotations.rs     Highlight / TextBox data, hit-testing, sidecar load/save
  export.rs          write an annotated copy of a PDF (late milestone)
  settings.rs        Settings struct, settings.toml load/save
  pdf/
    mod.rs
    worker.rs        the single pdfium thread: command loop
    types.rs         Command / Event enums, shared data types
  ui/
    tabbar.rs  pathbar.rs  toolbar.rs  sidebar_toc.rs
    findbar.rs viewer.rs   settings_tab.rs
scripts/setup_pdfium.sh
fixtures/            small test PDFs supplied by the owner
PROGRESS.md          you maintain this
ARCHITECTURE.md      you write this in Milestone 0 and keep it accurate
```

### Worker thread protocol

- UI → worker: `Command` over a `crossbeam` channel. Worker → UI: `Event` over another channel. After sending an event, the worker calls `ctx.request_repaint()`.
- Commands (extend as milestones need): `Open{doc_id, path}`, `Close{doc_id}`, `Render{doc_id, page, width_px, rotation, generation, priority}`, `Outline{doc_id}`, `PageText{doc_id, page}`, `Search{doc_id, query_id, query}`, `Export{...}`.
- Every `Render` carries a `generation` number that the UI bumps whenever scroll, zoom or rotation changes. The worker skips requests whose generation is older than the latest it has seen for that doc. The UI drops results that arrive for a stale generation.
- Long jobs (search, text indexing) are **chunked per page**. Between pages the worker checks for pending commands (`try_recv`) and serves `Render` first. A new `Search` for the same doc cancels the old one.
- The worker owns all `PdfDocument` handles in a `HashMap<doc_id, PdfDocument>`.

### Coordinate system (a classic bug source: follow exactly)

- Pdfium page space is in points with the origin at the bottom-left. **Convert once, at the worker boundary,** to our canonical space: **points, origin top-left, unrotated page.**
- All stored data (text char boxes, search hits, highlights, text boxes) is in canonical space.
- Rotation (0/90/180/270) and zoom are **view-only**. One helper, `PageTransform`, maps canonical ↔ screen for a given zoom, rotation, page size and screen origin. Every draw and hit-test goes through it. Annotations must rotate with the page.

### Rendering and cache

- Layout is virtualized: the worker returns all page sizes at open (cheap). The UI computes page rectangles from sizes, zoom and rotation, and only draws and requests pages intersecting the viewport (+1 page of prefetch on each side).
- Render at `zoom * pixels_per_point` target width, and cap any texture side at 4096 px.
- Progressive: request a low-res render (~35% scale) first for instant paint, then the sharp one, and swap.
- `cache.rs`: LRU keyed by `(doc_id, page, width_px, rotation)`, hard cap **150 MB** of texture bytes, evicting and freeing the egui textures. Switching tabs drops that tab's textures; the open document handle may stay.
- Ask pdfium for BGRA/RGBA directly and upload without extra copies where possible.

## Feature spec (the product)

Window layout, top to bottom: **tab bar → path bar (collapsible) → toolbar → body (TOC sidebar | page view)**. Dark UI chrome, white pages. Minimal and uncluttered, with no animations beyond what is needed.

- **Path bar:** one text field. The user pastes a file path and presses Enter (or clicks Open) to open it in a **new tab**. Accept quoted paths, `~`, and `file://` prefixes; trim whitespace. Show an inline error for invalid or non-PDF paths. A chevron button collapses and expands the bar. `Ctrl+L` expands and focuses it.
- **Tabs:** multiple PDFs, each tab keeping its own scroll, zoom, rotation, find state and annotations. Close button on each tab; middle-click and `Ctrl+W` close; `Ctrl+Tab` / `Ctrl+Shift+Tab` cycle. The Settings tab is a tab like any other (`Tab::Settings`).
- **TOC sidebar:** toggle button at the toolbar's left. Shows the PDF outline (bookmarks) as a collapsible tree; clicking jumps to the page. If there is no outline, show "No table of contents".
- **Highlighter:** toolbar button with a dropdown containing a **color chooser** (6 preset colors) and a **thickness chooser** (slider, 25%–100% of the line height, default 100%; the highlight band is vertically centered on the line). Drag over text to highlight. Drawn as a translucent overlay (default alpha ~0.35) so text stays readable.
- **Eraser:** toolbar toggle. Click a highlight (or drag across it) to delete it. Also deletes text boxes on click.
- **Text tool:** toolbar toggle. Click on a page to place a text box and type inline; click outside to commit; empty boxes are discarded. Existing boxes can be moved by dragging, edited by double-click, and deleted with `Delete`. A font size dropdown and color are shown while the tool is active.
- **Selection:** with no tool active, drag to select text across lines and pages; `Ctrl+C` copies it.
- **Zoom:** `-` / `+` buttons, percentage display, `Ctrl+scroll`, `Ctrl+=`, `Ctrl+-`, `Ctrl+0` (fit width), and a fit-page button. Clamp to 10%–800%.
- **Page box:** editable "page N of M"; Enter jumps. Updates as you scroll.
- **Rotate:** button rotates all pages of the tab 90° clockwise (view only).
- **Find:** `Ctrl+F` opens a find bar: text box, match count ("3 of 27"), next/previous (`Enter` / `Shift+Enter`), close (`Esc`). Case-insensitive. Matches are highlighted on the pages, the current one emphasized and scrolled into view. Search runs on the worker, streams results page by page, and never freezes the UI.
- **Fullscreen:** button and `F11`. Hides tab bar, path bar, toolbar and sidebar; `Esc` or `F11` exits.
- **Settings (gear icon):** opens a Settings tab (reusing the existing one if open). Content is only a placeholder for now; the owner will specify it later. Settings persist in `~/.config/pdfview/settings.toml`.
- Keyboard scrolling: arrows, `PgUp`/`PgDn`, `Home`/`End`, `Space`.

### Annotation storage

- Highlights and text boxes are stored by us, in canonical coordinates, in `~/.local/share/pdfview/annotations/<blake3(abs path + file size)>.json`. Autosave is debounced ~500 ms after a change, and also runs on tab close and app exit. The user's PDF is never modified.
- Real PDF annotations (`/Highlight`, `/FreeText`) are written only by the **Export** action, to a new file `<name>_annotated.pdf`. Verify that exported annotations actually render when the file is reopened in another viewer.

## Performance budget (measure, don't assume)

- Cold start to first painted page: under 300 ms on a normal PDF.
- Smooth 60 fps scrolling on a 500+ page book.
- RSS under ~200 MB with a 500-page book open at 100% zoom (measure with `ps -o rss= -p <pid>` or `/usr/bin/time -v`).
- Idle CPU ~0%: egui repaints only when something changes (no continuous `request_repaint`).
- No per-frame heap allocations in the viewer hot path where avoidable.

## Workflow for every milestone

1. Read `GEMINI.md` and `PROGRESS.md`.
2. Briefly state your plan (the files you will touch). Keep it short.
3. Implement only the named milestone.
4. Run, in this order, and fix everything: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo build --release`, `cargo test`.
5. Where possible, add small automated tests for non-UI logic (coordinate transforms, path parsing, annotation hit-testing, worker open/render/outline/search against `fixtures/`).
6. Update `PROGRESS.md`: what is done, what is known-broken, and the manual checks the owner should perform. Update `ARCHITECTURE.md` if the structure changed.
7. Tell the owner exactly how to run it and what to check. Then stop and wait.
8. Do not run `git commit` yourself. The owner commits.

## Style

Idiomatic, readable Rust. Small functions, clear names, `Result` for fallible paths with human-readable errors shown in the UI. No `unwrap()` on user-facing paths. Comments explain *why*, not *what*.
