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
    fn CGMainDisplayID() -> u32;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut std::ffi::c_void;
    fn CGDisplayModeGetRefreshRate(mode: *mut std::ffi::c_void) -> f64;
    fn CGDisplayModeRelease(mode: *mut std::ffi::c_void);
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

/// Refresh rate of the main display (the one with the menu bar), rounded to whole hertz.
/// None when CoreGraphics doesn't report one, as with some fixed-rate built-in panels.
pub fn main_refresh_rate() -> Option<u32> {
    // SAFETY: the mode is released after reading it; a null mode is never dereferenced.
    let rate = unsafe {
        let mode = CGDisplayCopyDisplayMode(CGMainDisplayID());
        if mode.is_null() {
            return None;
        }
        let rate = CGDisplayModeGetRefreshRate(mode);
        CGDisplayModeRelease(mode);
        rate
    };
    (rate >= 1.0).then(|| rate.round() as u32)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Time per call of the checks the menu bar app polls:
    /// `cargo test --release poll_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn poll_cost() {
        fn time<T>(name: &str, mut check: impl FnMut() -> T) {
            const CALLS: u32 = 2000;
            check();
            let started = Instant::now();
            for _ in 0..CALLS {
                std::hint::black_box(check());
            }
            println!("{name}: {:?} per call", started.elapsed() / CALLS);
        }
        time("main_refresh_rate", main_refresh_rate);
        time("built_in_online", built_in_online);
        time("supervisor_pid", supervisor_pid);
    }
}
