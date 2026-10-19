#[cfg(target_os = "linux")]
pub mod wayland;

#[cfg(unix)]
pub mod ipc;

pub mod dialog;
