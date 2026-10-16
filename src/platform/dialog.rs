use std::path::PathBuf;
use std::process::Command;

/// Launches a native Linux file chooser dialog to select a PDF file.
///
/// Uses the system's desktop environment file picker:
/// 1. `zenity` (standard on GNOME, Ubuntu, XFCE)
/// 2. `kdialog` (standard on KDE Plasma)
///
/// Returns `Some(PathBuf)` if a file was selected, or `None` if dismissed,
/// cancelled, or if neither tool is installed.
pub fn pick_pdf_file_native() -> Option<PathBuf> {
    // 1. Try zenity (standard on Ubuntu & GNOME desktop)
    let zenity_res = Command::new("zenity")
        .arg("--file-selection")
        .arg("--title=Open PDF Document — Verso Reader")
        .arg("--file-filter=PDF files (*.pdf) | *.pdf *.PDF")
        .output();

    if let Ok(output) = zenity_res {
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                let path = PathBuf::from(path_str);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        // Exit code 1 indicates the user explicitly cancelled the dialog
        if output.status.code() == Some(1) {
            return None;
        }
    }

    // 2. Fallback to kdialog (standard on KDE Plasma)
    let kdialog_res = Command::new("kdialog")
        .arg("--getopenfilename")
        .arg(".")
        .arg("*.pdf *.PDF | PDF files (*.pdf)")
        .arg("--title")
        .arg("Open PDF Document — Verso Reader")
        .output();

    if let Ok(output) = kdialog_res {
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                let path = PathBuf::from(path_str);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        if output.status.code() == Some(1) {
            return None;
        }
    }

    None
}
