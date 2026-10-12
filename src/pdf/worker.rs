use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use pdfium_render::prelude::*;

use crate::pdf::types::{Command, Event};

/// Computes the candidate file paths to search for `libpdfium.so`, in priority order:
/// 1. Directory of the executable
/// 2. `<exe dir>/lib`
/// 3. `./lib`
pub fn candidate_library_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. Directory of the executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            paths.push(exe_dir.join(Pdfium::pdfium_platform_library_name_at_path("")));
            // 2. <exe dir>/lib
            paths.push(
                exe_dir
                    .join("lib")
                    .join(Pdfium::pdfium_platform_library_name_at_path("")),
            );
        }
    }

    // 3. ./lib
    paths.push(PathBuf::from("./lib").join(Pdfium::pdfium_platform_library_name_at_path("")));

    paths
}

/// Attempts to bind to Pdfium using the prioritized search order:
/// Directory of executable -> `<exe dir>/lib` -> `./lib` -> System library.
pub fn bind_pdfium() -> Result<&'static Pdfium, String> {
    let candidates = candidate_library_paths();
    let mut searched: Vec<String> = Vec::new();

    for path in &candidates {
        searched.push(path.display().to_string());
        if path.exists() {
            if let Ok(bindings) = Pdfium::bind_to_library(path) {
                let pdfium = Box::new(Pdfium::new(bindings));
                return Ok(Box::leak(pdfium));
            }
        }
    }

    // 4. System library
    searched.push("System library search path".to_string());
    if let Ok(bindings) = Pdfium::bind_to_system_library() {
        let pdfium = Box::new(Pdfium::new(bindings));
        return Ok(Box::leak(pdfium));
    }

    Err(format!(
        "Could not load libpdfium.so.\n\nSearched locations:\n  • {}\n\nPlease run `bash scripts/setup_pdfium.sh` to download libpdfium.so into ./lib/.",
        searched.join("\n  • ")
    ))
}

/// Spawns the dedicated pdfium worker thread and returns communication channels.
pub fn start_worker(ctx: egui::Context) -> (Sender<Command>, Receiver<Event>) {
    let (tx_cmd, rx_cmd) = crossbeam_channel::unbounded::<Command>();
    let (tx_evt, rx_evt) = crossbeam_channel::unbounded::<Event>();

    thread::Builder::new()
        .name("pdfview-worker".to_string())
        .spawn(move || {
            worker_main(ctx, rx_cmd, tx_evt);
        })
        .expect("Failed to spawn pdfview-worker thread");

    (tx_cmd, rx_evt)
}

fn worker_main(ctx: egui::Context, rx_cmd: Receiver<Command>, tx_evt: Sender<Event>) {
    // Attempt binding to Pdfium
    let _pdfium = match bind_pdfium() {
        Ok(instance) => {
            let _ = tx_evt.send(Event::PdfiumReady);
            ctx.request_repaint();
            Some(instance)
        }
        Err(err) => {
            let _ = tx_evt.send(Event::PdfiumError(err));
            ctx.request_repaint();
            None
        }
    };

    // Command loop: will handle Open, Close, Render, etc. in future milestones
    #[allow(clippy::never_loop)]
    while let Ok(cmd) = rx_cmd.recv() {
        match cmd {
            Command::Shutdown => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_paths_not_empty() {
        let paths = candidate_library_paths();
        assert!(!paths.is_empty());
        // Verify ./lib/libpdfium.so is included
        assert!(paths.iter().any(|p| p.ends_with("libpdfium.so")
            || p.ends_with("pdfium.dll")
            || p.ends_with("libpdfium.dylib")));
    }

    #[test]
    fn test_bind_pdfium_with_local_lib() {
        let res = bind_pdfium();
        assert!(
            res.is_ok(),
            "Expected bind_pdfium to succeed with ./lib/libpdfium.so: {:?}",
            res.err()
        );
    }
}
