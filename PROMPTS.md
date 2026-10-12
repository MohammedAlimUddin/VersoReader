# pdfview — build playbook

You oversee; Gemini codes. Each milestone below has a **prompt to paste** into Gemini CLI and a **checklist for you** to verify before committing and moving on.

## Before you start (once)

```bash
# Rust toolchain and system libraries
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
sudo apt install -y build-essential pkg-config clang libclang-dev cmake curl \
  libfontconfig1-dev libx11-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev

# Gemini CLI (needs Node.js 20+); log in with the Google account that has Gemini Pro
npm install -g @google/gemini-cli

# Project folder
mkdir -p ~/pdfview && cd ~/pdfview
git init
mkdir fixtures scripts
```

1. Copy `GEMINI.md` into `~/pdfview/` (project root).
2. Put 3 test PDFs in `~/pdfview/fixtures/`: a **small** one (5–20 pages), a **large book with a table of contents** (300+ pages), and one **scanned/image-only** PDF.
3. Create a `.gitignore` containing `target/` and `lib/` (plus `fixtures/` if your test PDFs are copyrighted books).
4. Run `gemini` inside `~/pdfview`, choose the Pro model, and run `/memory show` to confirm `GEMINI.md` is loaded.

### Your loop for every milestone    

1. Paste the milestone prompt into Gemini.
2. When it says done, run `cargo run --release -- fixtures/<some>.pdf` yourself and go through the checklist.
3. If something fails, paste the exact error or describe the symptom to Gemini ("Fix only this, don't touch anything else").
4. When the checklist passes: `git add -A && git commit -m "Milestone N"`.
5. Only then move on. If Gemini starts drifting (rewriting files, adding features), `git checkout .` and re-prompt with "minimal diff, only milestone N".

---

## Milestone 0 — Scaffold, Pdfium, architecture doc

```text
Read GEMINI.md fully. Implement Milestone 0 only.

1. Create the Cargo project in the current directory (binary name pdfview) with the dependencies and release profile from GEMINI.md. Check the latest pdfium-render version on docs.rs and pin it exactly. Check eframe's current feature names and use the glow backend with X11 and Wayland support.
2. Write scripts/setup_pdfium.sh that downloads pdfium-linux-x64.tgz from the latest bblanchon/pdfium-binaries release and puts libpdfium.so into ./lib/. Add .gitignore (target/, lib/).
3. Create the module skeleton from the Architecture section (empty or minimal modules are fine) and the worker thread with the Command/Event channel plumbing. The worker binds to Pdfium using the search order in GEMINI.md and reports success or a clear error via an Event.
4. main.rs opens an eframe window titled "pdfview" with a dark theme that shows either "Pdfium ready" or the bind error message.
5. Write ARCHITECTURE.md describing modules, threads, channels and the coordinate system, and create PROGRESS.md with a Milestone 0 entry.

Then follow the Workflow section of GEMINI.md and stop.
```

**You check**
- [ ] `bash scripts/setup_pdfium.sh` puts `libpdfium.so` in `lib/`.
- [ ] `cargo run --release` opens a dark window saying Pdfium is ready.
- [ ] Rename `lib/` temporarily: the window shows a readable error instead of crashing.
- [ ] `ARCHITECTURE.md` exists and matches GEMINI.md.

---

## Milestone 1 — Open a PDF and render page 1

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 1 only.

- Add the path bar at the top: a single text field plus an Open button. Pressing Enter or clicking Open sends Command::Open with the path. Accept quoted paths, "~", and "file://" prefixes, and trim whitespace. Show an inline error for missing or non-PDF files, and for pdfium load errors (e.g. password-protected).
- Worker: on Open, load the document with load_pdf_from_file, reply with the page count and ALL page sizes in canonical points. On Render, render the requested page to a bitmap at the requested pixel width and send it back as an Event.
- UI: show page 1 of the opened document, fit to the window width, as an egui texture.
- Also accept an optional CLI argument: "pdfview path/to/file.pdf" opens that file on start.
- Add worker tests against fixtures/ (open, page count, render page 0 returns non-empty bitmap) and unit tests for path parsing.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Paste a full path of your large book: page 1 appears quickly and looks identical in quality to Edge.
- [ ] Paste the path with quotes and with `~`: works.
- [ ] Paste a wrong path: inline error, no crash.
- [ ] The UI stays responsive while the large book opens.

---

## Milestone 2 — Continuous scrolling, lazy rendering, cache

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 2 only.

- Continuous vertical scrolling through all pages in a ScrollArea-like viewer. Layout is virtualized from the page sizes: only pages intersecting the viewport plus 1 page of prefetch each side are requested and drawn. Pages are centered, with a small gap between them.
- Render requests carry a generation number that is bumped on scroll/zoom changes; the worker skips stale ones; the UI drops stale results.
- Progressive rendering: low-res (~35%) first, then the sharp render, swapped in when ready. Draw a light-gray placeholder rectangle while nothing is available.
- Implement cache.rs: LRU of textures with a hard 150 MB byte cap, evicting and freeing textures.
- Page box in a minimal toolbar: editable "N of M" that updates as you scroll; Enter jumps to the page.
- Keyboard scrolling: arrows, PgUp/PgDn, Home/End, Space.
- egui must repaint only when something changes (no continuous repaint).
- Add a debug line (toggle with F3) showing cache size in MB, number of cached pages and frame time.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Open the 300+ page book, drag the scrollbar rapidly to the middle and the end: pages fill in quickly, no freeze.
- [ ] Press `F3`: cache size never exceeds ~150 MB after a long scroll.
- [ ] `ps -o rss= -p $(pgrep pdfview)` stays reasonable (target under ~200 MB at 100% zoom).
- [ ] Idle: `top` shows ~0% CPU when you aren't scrolling.
- [ ] Page box updates while scrolling; typing a number and Enter jumps there.

---

## Milestone 3 — Zoom, rotate, fullscreen

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 3 only.

- Zoom: "-" and "+" toolbar buttons, a percentage display, Ctrl+scroll, Ctrl+= / Ctrl+- / Ctrl+0 (fit width), and a fit-page button. Clamp 10%-800%. Zoom around the cursor (or viewport center for buttons) so the content under it stays in place. Cap texture sides at 4096 px as specified.
- Rotate: a toolbar button rotating all pages 90 degrees clockwise (view-only). Introduce the PageTransform helper in GEMINI.md's coordinate section now and use it for layout and drawing, with unit tests for all four rotations.
- Fullscreen: a toolbar button and F11 toggle; in fullscreen hide all chrome; Esc or F11 exits.
- Keep scroll position sensible (stay on the same page) across zoom and rotation changes.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Zoom with buttons, Ctrl+scroll and keyboard; the text stays sharp after the sharp render swaps in.
- [ ] 800% zoom doesn't blow up RAM.
- [ ] Rotate four times returns to the original; scroll position stays on the same page.
- [ ] `F11` goes fullscreen and back; `Esc` exits.

---

## Milestone 4 — Tabs and collapsible path bar

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 4 only.

- Add the tab bar above the path bar. Introduce `enum Tab { Pdf(PdfTab), Settings }` in app.rs. Each PdfTab keeps its own doc_id, scroll, zoom, rotation and page-box state. Opening a path from the path bar opens a NEW tab and focuses it. A "+" button focuses the path bar.
- Tab titles are the file name (truncated with ellipsis); tooltip shows the full path. Close button on each tab, middle-click closes, Ctrl+W closes, Ctrl+Tab and Ctrl+Shift+Tab cycle.
- Closing a tab sends Command::Close and frees that tab's cached textures. Switching tabs drops the previous tab's textures from the cache (keep the document handle).
- Make the path bar collapsible with a chevron button; Ctrl+L expands and focuses it.
- Keep memory bounded: opening and closing 10 tabs must not leak.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Open 3 different PDFs; each remembers its own scroll, zoom and rotation when you switch back.
- [ ] Closing tabs reduces RSS; open/close the same book 10 times: RSS doesn't keep climbing.
- [ ] Collapse and expand the path bar; `Ctrl+L` works.
- [ ] Middle-click, `Ctrl+W`, `Ctrl+Tab` behave.

---

## Milestone 5 — Table of contents sidebar

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 5 only.

- Worker: on Outline, read the document's bookmarks recursively (title, destination page index, children) and send them back as a tree. Verify the pdfium-render bookmarks API in the pinned version's docs first. Handle bookmarks without a resolvable page gracefully (show but don't jump).
- UI: a toggle button at the toolbar's left opens a left sidebar (resizable, ~280 px default) with a collapsible tree. Clicking an entry scrolls to that page. Highlight the entry of the current page's chapter if cheap to compute.
- Empty case: show "No table of contents".
- Request the outline once at open, per tab; the sidebar state (open/closed, expanded nodes) is per tab.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Large book: chapter tree matches Edge's sidebar; clicking jumps to the correct page.
- [ ] A PDF without bookmarks shows the empty message.
- [ ] Sidebar state is remembered per tab.

---

## Milestone 6 — Find

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 6 only.

- Ctrl+F opens a find bar below the toolbar: text box (focused), match count "3 of 27", previous/next buttons (Enter / Shift+Enter), close (Esc).
- Worker: Command::Search{doc_id, query_id, query}. Search page by page, case-insensitive, using pdfium's text search (verify the API in the pinned docs). Return matches in canonical coordinates as rects, streamed per page so results appear progressively. Between pages, check for pending Render commands and serve them first; a newer Search cancels the older one.
- UI: debounce typing (~150 ms), show a running count while the search progresses, draw match rectangles on pages through PageTransform (all matches in a soft color, the current one in a stronger color), and scroll the current match into view when navigating.
- Per-tab find state. Searching must never freeze scrolling or rendering.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Search a common word in the 300+ page book: the first hits appear almost instantly, the count keeps increasing.
- [ ] Scrolling stays smooth while the search is still running.
- [ ] Next/previous wraps around and scrolls to the match; highlights land exactly on the words, also when zoomed and rotated.
- [ ] `Esc` closes and clears highlights.

---

## Milestone 7 — Text layer and selection

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 7 only.

- Worker: Command::PageText{doc_id, page} returns the page's characters with their boxes in canonical coordinates (verify the text/chars API in the pinned docs), plus line grouping so selections can be merged into one rectangle per line. Cache a small number of recent pages' text in the UI (cap it).
- UI: with no tool active, dragging over text selects characters (cursor becomes a text I-beam over text). Selection works across lines and across pages, drawn as translucent blue rectangles per line. Ctrl+C copies the selected text with sensible line breaks. Click elsewhere clears the selection.
- All hit-testing goes through PageTransform and must be correct at any zoom and rotation. Add unit tests for hit-testing and selection ordering.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Select a paragraph, then multiple lines across a page boundary: highlight shapes look right.
- [ ] `Ctrl+C` then paste into a text editor: text is correct and in reading order.
- [ ] Works while zoomed and rotated.
- [ ] Two-column PDFs: selection stays within the column you dragged in as best as pdfium allows (note any problems for later).

---

## Milestone 8 — Highlighter and eraser (core feature)

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 8 only.

- Data (annotations.rs): Highlight { page, rects (canonical coords, one per line), color, alpha, thickness } stored per document and persisted as JSON in ~/.local/share/pdfview/annotations/<blake3(abs path + file size)>.json. Debounced autosave (~500 ms) plus save on tab close and exit. Load on open.
- Toolbar: a highlighter button with a dropdown containing a 6-color preset chooser and a thickness slider (25%-100% of line height, default 100%, band vertically centered on each line). The chosen color and thickness persist in settings.toml via settings.rs (create it minimally).
- Behaviour: with the highlighter active, dragging over text creates a highlight from the selected characters on release (reuse Milestone 7 selection). Draw highlights as translucent overlays (alpha ~0.35) above the page texture and below nothing else, through PageTransform. Highlights must stay correct when zooming and rotating.
- Eraser toolbar toggle: clicking a highlight deletes it; dragging across highlights deletes each one touched. Cursor shows the active tool.
- Ctrl+Z / Ctrl+Y undo/redo for highlight create and delete (per tab, bounded history).
- Tests: serialization round-trip, hit-testing, thickness rect math.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Highlight text in each of the 6 colors; thickness slider visibly changes the band.
- [ ] Close the tab, reopen the same PDF: highlights are back.
- [ ] Zoom and rotate: highlights stay glued to the words.
- [ ] Eraser removes only what you click; `Ctrl+Z` brings it back.
- [ ] The original PDF file's modification time is unchanged.

---

## Milestone 9 — Text tool

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 9 only.

- Data: TextBox { page, position (canonical), text, font_size, color, width } stored in the same annotation JSON as highlights (bump a schema version field).
- Toolbar: a text-tool toggle; while active show a font-size dropdown (e.g. 10-36) and a color chooser.
- Behaviour: click on a page to create a box and type inline (multi-line; Enter inserts a newline); click outside to commit; empty boxes are discarded. With the tool active (or any tool inactive) boxes can be dragged to move, double-clicked to edit, and deleted with Delete. The eraser also deletes a box on click. Draw boxes through PageTransform so they rotate and scale with the page; text rendering size scales with zoom.
- Undo/redo covers text box create/edit/move/delete.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Place, edit, move, and delete text boxes; they persist after closing and reopening.
- [ ] Boxes scale with zoom and rotate with the page.
- [ ] Undo/redo works.

---

## Milestone 10 — Settings tab

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 10 only.

- Add the gear icon at the far right of the toolbar. Clicking it opens the Settings tab (Tab::Settings) or focuses it if it is already open. It closes like any tab.
- settings_tab.rs: a simple page with a title "Settings" and a placeholder text "Settings will appear here." Structure the code so adding a labeled section with controls later is a few lines (e.g. a small helper for a setting row).
- Settings struct in settings.rs is persisted to ~/.config/pdfview/settings.toml with serde defaults, so adding fields later never breaks old files. Currently it holds the highlighter color/thickness, TOC sidebar visibility, and path bar collapsed state; restore these on launch.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Gear opens exactly one Settings tab; closing and reopening works.
- [ ] Restart the app: highlighter color/thickness, sidebar and path-bar state are restored.
- [ ] Delete `settings.toml`: the app starts normally with defaults.

---

## Milestone 11 — Export annotated PDF

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 11 only.

- Add an "Export with annotations" action (a small menu entry reachable from the gear area or a keyboard shortcut Ctrl+S; tell me where you put it).
- Worker: Command::Export{doc_id, annotations, out_path}. Open the original, add real /Highlight annotations (one quad per line rect, with color and alpha) and /FreeText annotations for text boxes using the pinned pdfium-render API, convert canonical coordinates back to PDF space, and save to "<name>_annotated.pdf" next to the original (never overwrite the original; pick a non-clashing name).
- IMPORTANT: verify that the annotations actually RENDER when the exported file is reopened (in pdfview and in another viewer such as Evince/Okular/Firefox). If pdfium does not draw them without appearance streams, find a documented way to generate them, or tell me clearly what is not possible instead of faking success.
- Show success or an error message in the UI.

Then follow the Workflow section and stop.
```

**You check**
- [ ] Export, then open the exported PDF in Firefox/Edge: highlights and text boxes are visible and positioned correctly.
- [ ] The original PDF is untouched; exporting twice doesn't overwrite earlier exports.
- [ ] If Gemini reports a limitation, decide with it whether to keep export simple or drop it.

---

## Milestone 12 — Performance audit, polish, install

```text
Read GEMINI.md and PROGRESS.md. Implement Milestone 12 only.

1. Performance audit against the budget in GEMINI.md. Measure and report real numbers: cold start to first page, RSS with the 500-page book at 100% and 300% zoom, RSS after opening and closing 10 tabs, idle CPU, frame times while scrolling fast. Fix anything that misses the budget (obvious candidates: texture upload copies, per-frame allocations, over-eager prefetch, cache eviction freeing textures).
2. Run clippy with -D warnings and remove any remaining unwrap()/panics on user-facing paths. Make sure corrupt or encrypted PDFs show a clear message instead of crashing.
3. Add a README.md with build and run instructions, shortcuts table, and where data is stored.
4. Add scripts/install.sh that builds in release mode and installs the binary to ~/.local/bin/pdfview, copies lib/libpdfium.so next to it (or into ~/.local/lib/pdfview/ with the binary finding it), and installs a pdfview.desktop file so it appears in the app launcher and can be chosen as the default PDF viewer.
5. Update ARCHITECTURE.md and PROGRESS.md.

Then follow the Workflow section and stop. Report the measured numbers in your final message.
```

**You check**
- [ ] The reported numbers meet the budget; you confirm with `ps` yourself.
- [ ] After running `scripts/install.sh`, `pdfview file.pdf` works from any directory and the app shows in the launcher.
- [ ] Right-click a PDF in the file manager, "Open with" shows pdfview.

---

## If Gemini gets stuck

- **Compile errors it can't fix after two tries:** tell it "Stop guessing. Read the docs.rs page for this exact type in the pinned version and the source in ~/.cargo/registry, then fix."
- **Wrong or invented API:** paste the compiler error and add "Check the pinned pdfium-render version's docs; do not use memory."
- **It touches unrelated files:** `git checkout .` and re-run the prompt with "minimal diff, only the files needed for this milestone."
- **Pdfium crashes or hangs:** confirm every pdfium call is on the worker thread; ask Gemini to audit for any pdfium use outside `pdf/worker.rs`.
- **Context gets long and sloppy:** start a fresh `gemini` session; it re-reads `GEMINI.md` and `PROGRESS.md` and continues from the last committed milestone.
