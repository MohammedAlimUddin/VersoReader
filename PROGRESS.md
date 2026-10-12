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
  - [ ] `bash scripts/setup_pdfium.sh` puts `libpdfium.so` into `lib/`.
  - [ ] `cargo run --release` opens a dark window saying "Pdfium ready".
  - [ ] Rename `lib/` temporarily: the window displays a readable error without crashing.
  - [ ] `ARCHITECTURE.md` exists and accurately describes the system.

