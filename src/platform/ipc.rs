//! Single-instance IPC over a Unix domain socket.
//!
//! When another instance of Verso Reader is already running:
//! - If launched with a file argument, sends an `OPEN <path>` command to open the file in a new tab.
//! - If launched without arguments, sends a `FOCUS` command to bring the existing window to the front.
//!
//! In both cases, the second process exits cleanly immediately without spawning a duplicate window.

use crossbeam_channel::{Receiver, Sender};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Returns the standard Unix socket path for Verso Reader.
///
/// Follows the XDG Base Directory specification:
/// 1. Uses `$XDG_RUNTIME_DIR/verso-reader.sock` if `$XDG_RUNTIME_DIR` is set.
/// 2. Falls back to `/tmp/verso-reader-<username>.sock`.
pub fn socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let trimmed = runtime_dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("verso-reader.sock");
        }
    }
    let username = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
    std::env::temp_dir().join(format!("verso-reader-{}.sock", username))
}

/// Commands received by the running application from secondary instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcMessage {
    /// Open the specified document in a new tab (or focus existing tab).
    Open(PathBuf),
    /// Focus and bring the application window to the foreground.
    Focus,
}

/// Attempts to connect to an existing instance and send an OPEN request.
/// Returns true if the message was delivered and acknowledged with "OK".
pub fn try_send_open(path: &Path) -> bool {
    let sock = socket_path();
    send_ipc_command(&sock, &format!("OPEN {}\n", path.display()))
}

/// Attempts to connect to an existing instance and send a FOCUS request.
/// Returns true if the message was delivered and acknowledged with "OK".
pub fn try_send_focus() -> bool {
    let sock = socket_path();
    send_ipc_command(&sock, "FOCUS\n")
}

/// Internal helper to send a command line to the given socket path.
fn send_ipc_command(sock_path: &Path, command: &str) -> bool {
    let mut stream = match UnixStream::connect(sock_path) {
        Ok(s) => s,
        Err(err) => {
            // If the socket file exists but connection was refused, the previous process
            // died without unlinking the socket. Clean up the dead socket file.
            if err.kind() == std::io::ErrorKind::ConnectionRefused {
                let _ = std::fs::remove_file(sock_path);
            }
            return false;
        }
    };

    let timeout = Duration::from_millis(800);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    if stream.write_all(command.as_bytes()).is_err() {
        return false;
    }
    if stream.flush().is_err() {
        return false;
    }

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).is_ok() && response.trim() == "OK"
}

/// The IPC server running in the primary application instance.
pub struct IpcServer {
    socket_path: PathBuf,
    is_running: Arc<AtomicBool>,
}

impl IpcServer {
    /// Attempts to bind to the socket path and start listening in a background thread.
    ///
    /// If an active server is already listening, returns None.
    /// If a stale socket file exists, it is cleaned up and rebound.
    pub fn start(ctx: egui::Context) -> Option<(Self, Receiver<IpcMessage>)> {
        Self::start_with_path(socket_path(), ctx)
    }

    /// Starts an IPC server with a specific socket path (useful for testing).
    pub fn start_with_path(
        socket_path: PathBuf,
        ctx: egui::Context,
    ) -> Option<(Self, Receiver<IpcMessage>)> {
        // First try to bind directly.
        let listener = match UnixListener::bind(&socket_path) {
            Ok(l) => l,
            Err(err) => {
                if err.kind() == std::io::ErrorKind::AddrInUse {
                    // Probe if the socket is actually active or stale
                    match UnixStream::connect(&socket_path) {
                        Ok(mut probe_stream) => {
                            let _ =
                                probe_stream.set_write_timeout(Some(Duration::from_millis(300)));
                            let _ = probe_stream.set_read_timeout(Some(Duration::from_millis(300)));
                            if probe_stream.write_all(b"PING\n").is_ok() {
                                let mut reader = BufReader::new(probe_stream);
                                let mut reply = String::new();
                                if reader.read_line(&mut reply).is_ok() && reply.trim() == "PONG" {
                                    // A valid server is already active!
                                    return None;
                                }
                            }
                        }
                        Err(_) => {
                            // Connection failed, so socket is stale. Delete and re-bind.
                            let _ = std::fs::remove_file(&socket_path);
                        }
                    }

                    // Try binding again after removing stale socket
                    UnixListener::bind(&socket_path).ok()?
                } else {
                    return None;
                }
            }
        };

        let (tx, rx) = crossbeam_channel::unbounded();
        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_clone = is_running.clone();

        std::thread::spawn(move || {
            Self::listener_loop(listener, tx, ctx, is_running_clone);
        });

        Some((
            Self {
                socket_path,
                is_running,
            },
            rx,
        ))
    }

    fn listener_loop(
        listener: UnixListener,
        tx: Sender<IpcMessage>,
        ctx: egui::Context,
        is_running: Arc<AtomicBool>,
    ) {
        while is_running.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(1000)));

                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_ok() {
                        let trimmed = line.trim();
                        if trimmed == "PING" {
                            let _ = stream.write_all(b"PONG\n");
                            let _ = stream.flush();
                        } else if trimmed == "FOCUS" {
                            let _ = tx.send(IpcMessage::Focus);
                            let _ = stream.write_all(b"OK\n");
                            let _ = stream.flush();
                            ctx.request_repaint();
                        } else if let Some(path_str) = trimmed.strip_prefix("OPEN ") {
                            let p = PathBuf::from(path_str.trim());
                            let _ = tx.send(IpcMessage::Open(p));
                            let _ = stream.write_all(b"OK\n");
                            let _ = stream.flush();
                            ctx.request_repaint();
                        } else {
                            let _ = stream.write_all(b"ERR Unknown command\n");
                            let _ = stream.flush();
                        }
                    }
                }
                Err(_) => {
                    if !is_running.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_open_and_focus_communication() {
        let temp_dir = std::env::temp_dir();
        let sock_path = temp_dir.join(format!(
            "verso_test_ipc_{}_{}.sock",
            std::process::id(),
            fastrand_u32()
        ));

        let ctx = egui::Context::default();
        let (server, rx) =
            IpcServer::start_with_path(sock_path.clone(), ctx).expect("start test server");

        // Test 1: Send OPEN
        let sample_pdf = PathBuf::from("/sample/test document.pdf");
        assert!(send_ipc_command(
            &sock_path,
            &format!("OPEN {}\n", sample_pdf.display())
        ));
        let msg1 = rx
            .recv_timeout(Duration::from_millis(500))
            .expect("recv open");
        assert_eq!(msg1, IpcMessage::Open(sample_pdf));

        // Test 2: Send FOCUS
        assert!(send_ipc_command(&sock_path, "FOCUS\n"));
        let msg2 = rx
            .recv_timeout(Duration::from_millis(500))
            .expect("recv focus");
        assert_eq!(msg2, IpcMessage::Focus);

        // Test 3: Drop server unlinks socket file
        drop(server);
        assert!(!sock_path.exists(), "Socket file must be removed on drop");
    }

    fn fastrand_u32() -> u32 {
        use std::time::SystemTime;
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(42)
    }
}
