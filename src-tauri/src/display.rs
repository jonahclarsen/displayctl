//! Thin wrapper around the `displayctl` command-line supervisor.

use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is not set"))
}

pub fn log_dir() -> PathBuf {
    home().join("Library/Logs/displayctl")
}

/// The CLI built from this repository's `main.swift` (see `scripts/build-cli.sh`).
pub fn binary() -> PathBuf {
    std::env::var_os("DISPLAYCTL_BIN")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../displayctl"))
}

pub fn built_in_online() -> bool {
    let mut displays = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: the buffer holds `displays.len()` entries and CoreGraphics writes at most that many.
    let status = unsafe { CGGetOnlineDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count) };
    if status != 0 {
        log::warn!("CGGetOnlineDisplayList failed: {status}");
        return true;
    }
    displays[..count as usize].iter().any(|&id| unsafe { CGDisplayIsBuiltin(id) } != 0)
}

#[derive(serde::Deserialize)]
struct Recovery {
    #[serde(rename = "watchdogPID")]
    watchdog_pid: Option<i32>,
}

/// PID of a running `displayctl off` supervisor, whoever started it.
pub fn supervisor_pid() -> Option<i32> {
    let path = home().join(".local/state/displayctl/recovery.json");
    let pid = serde_json::from_slice::<Recovery>(&fs::read(path).ok()?).ok()?.watchdog_pid?;
    // SAFETY: signal 0 only checks that the process exists.
    let alive = pid > 1 && (unsafe { libc::kill(pid, 0) } == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM));
    alive.then_some(pid)
}

/// Starts the supervisor in its own process group so it keeps running (and keeps the
/// display state) across app restarts. Output goes to a file, never a pipe: a closed
/// pipe would kill it with SIGPIPE before it could restore the built-in display.
pub fn start_supervisor() -> io::Result<Child> {
    fs::create_dir_all(log_dir())?;
    let log = OpenOptions::new().create(true).append(true).open(log_dir().join("supervisor.log"))?;
    Command::new(binary())
        .arg("off")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
}

pub fn restore() -> io::Result<Output> {
    Command::new(binary()).arg("on").stdin(Stdio::null()).output()
}
