//! Thin wrapper around the `displayctl` command-line supervisor.

use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
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

// Match the Swift supervisor: a CPU spike can slow a 0.1 s probe past several
// seconds for over a minute, which is not evidence of an unplug.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const UNKNOWN_GRACE: Duration = Duration::from_secs(30);

#[derive(serde::Deserialize)]
struct Recovery {
    #[serde(rename = "builtInDisplayID")]
    built_in_display_id: u32,
    #[serde(rename = "watchdogPID")]
    watchdog_pid: Option<i32>,
    #[serde(default)]
    restored: bool,
    #[serde(default, rename = "automationSuspended")]
    automation_suspended: bool,
}

pub fn automation_suspended() -> bool {
    fs::read(home().join(".local/state/displayctl/recovery.json"))
        .ok()
        .and_then(|data| serde_json::from_slice::<Recovery>(&data).ok())
        .is_some_and(|record| record.automation_suspended)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetySnapshot {
    external_state: u8,
    built_in_usable: bool,
}

impl SafetySnapshot {
    pub fn unavailable() -> Self {
        Self { external_state: 3, built_in_usable: false }
    }

    pub fn can_resume_after_recovery(self) -> bool {
        self.external_state == 1
    }

    pub fn needs_recovery(self) -> bool {
        // Unknown still permits rescue, but must not restart automatic disabling.
        // 0 = ready; 1 = absent/unusable; 2 = asleep; 3 = probe unavailable.
        matches!(self.external_state, 1 | 3) && !self.built_in_usable
    }
}

/// Independent of the Swift supervisor's event queue and Process implementation.
pub fn safety_snapshot() -> io::Result<Option<SafetySnapshot>> {
    let path = home().join(".local/state/displayctl/recovery.json");
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let recovery: Recovery = serde_json::from_slice(&data)?;
    if recovery.restored {
        return Ok(None);
    }
    let mut command = Command::new(binary());
    command.args(["probe-display-state", &recovery.built_in_display_id.to_string()]);
    let output = run_with_timeout(&mut command, PROBE_TIMEOUT)?;
    if !output.status.success() {
        return Err(io::Error::other(format!("safety probe failed: {}", output.status)));
    }
    Ok(Some(serde_json::from_slice(&output.stdout)?))
}

#[derive(Default)]
pub struct UnplugWatchdog {
    missing_since: Option<Instant>,
    last_attempt: Option<Instant>,
    unknown_since: Option<Instant>,
    unknown_samples: u32,
}

impl UnplugWatchdog {
    pub fn poll(&mut self, snapshot: Option<SafetySnapshot>, now: Instant) -> bool {
        if !snapshot.is_some_and(SafetySnapshot::needs_recovery) {
            self.missing_since = None;
            self.last_attempt = None;
            self.unknown_since = None;
            self.unknown_samples = 0;
            return false;
        }
        let since = *self.missing_since.get_or_insert(now);
        if snapshot.is_some_and(|state| state.external_state == 3) {
            let unknown_since = *self.unknown_since.get_or_insert(now);
            self.unknown_samples = self.unknown_samples.saturating_add(1);
            // Match the Swift supervisor: isolated slow reads do not restore.
            if self.unknown_samples < 3 || now.duration_since(unknown_since) < UNKNOWN_GRACE {
                return false;
            }
        } else {
            self.unknown_since = None;
            self.unknown_samples = 0;
        }
        if now.duration_since(since) < Duration::from_secs(2)
            || self.last_attempt.is_some_and(|at| now.duration_since(at) < Duration::from_secs(10))
        {
            return false;
        }
        self.last_attempt = Some(now);
        true
    }
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
    let mut command = Command::new(binary());
    command.arg("on");
    // Supervisor shutdown (13s) plus targeted/fallback verification (up to 10s).
    run_with_timeout(&mut command, Duration::from_secs(26))
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("display helper timed out after {timeout:?}")));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn independent_watchdog_recovers_when_supervisor_never_reports_unplug() {
        let now = Instant::now();
        let absent = SafetySnapshot { external_state: 1, built_in_usable: false };
        let mut guard = UnplugWatchdog::default();
        assert!(!guard.poll(Some(absent), now));
        assert!(guard.poll(Some(absent), now + Duration::from_secs(2)));
        assert!(!guard.poll(Some(absent), now + Duration::from_secs(3)));
        assert!(guard.poll(Some(absent), now + Duration::from_secs(12)));
        // The supervisor's PID or heartbeat is deliberately not required.
        let restored = SafetySnapshot { external_state: 1, built_in_usable: true };
        assert!(!guard.poll(Some(restored), now + Duration::from_secs(13)));
        assert!(!guard.poll(Some(absent), now + Duration::from_secs(14)));
        assert!(guard.poll(Some(absent), now + Duration::from_secs(16)));
    }

    #[test]
    fn unknown_topology_allows_rescue_but_never_automatic_restart() {
        let unknown = SafetySnapshot::unavailable();
        let absent = SafetySnapshot { external_state: 1, built_in_usable: false };
        assert_ne!(unknown, absent);
        assert!(unknown.needs_recovery());
        assert!(!unknown.can_resume_after_recovery());
        assert!(absent.can_resume_after_recovery());
        let now = Instant::now();
        let mut watchdog = UnplugWatchdog::default();
        assert!(!watchdog.poll(Some(unknown), now));
        assert!(!watchdog.poll(Some(unknown), now + Duration::from_secs(2)));
        assert!(!watchdog.poll(Some(unknown), now + Duration::from_secs(6)));
        assert!(!watchdog.poll(Some(unknown), now + Duration::from_secs(29)));
        assert!(watchdog.poll(Some(unknown), now + Duration::from_secs(30)));
        // Failed restoration still retries, independently of the Swift worker.
        assert!(!watchdog.poll(Some(unknown), now + Duration::from_secs(36)));
        assert!(watchdog.poll(Some(unknown), now + Duration::from_secs(40)));
        assert!(!watchdog.poll(None, now + Duration::from_secs(41)));
    }

    #[test]
    fn isolated_slow_probes_do_not_restore_or_delay_confirmed_unplug() {
        let now = Instant::now();
        let mut guard = UnplugWatchdog::default();
        let unknown = SafetySnapshot::unavailable();
        let ready = SafetySnapshot { external_state: 0, built_in_usable: false };
        let absent = SafetySnapshot { external_state: 1, built_in_usable: false };
        assert!(!guard.poll(Some(unknown), now));
        assert!(!guard.poll(Some(ready), now + Duration::from_secs(1)));
        assert!(!guard.poll(Some(unknown), now + Duration::from_secs(10)));
        assert!(!guard.poll(Some(unknown), now + Duration::from_secs(20)));
        assert!(!guard.poll(Some(ready), now + Duration::from_secs(21)));
        assert!(!guard.poll(Some(absent), now + Duration::from_secs(22)));
        assert!(guard.poll(Some(absent), now + Duration::from_secs(24)));
    }

    #[test]
    fn cpu_spike_probe_timeouts_do_not_restore() {
        // Probe results logged during a CPU spike on 2026-10-16 (seconds from first miss).
        let now = Instant::now();
        let mut guard = UnplugWatchdog::default();
        let unknown = SafetySnapshot::unavailable();
        let ready = SafetySnapshot { external_state: 0, built_in_usable: false };
        for (at, snapshot) in [(0, unknown), (3, unknown), (5, ready), (9, unknown), (13, unknown),
                               (16, unknown), (20, unknown), (25, unknown), (30, ready)] {
            assert!(!guard.poll(Some(snapshot), now + Duration::from_secs(at)), "restored at {at}s");
        }
    }

    #[test]
    fn recovery_suspension_is_backward_compatible() {
        let old: Recovery = serde_json::from_str(r#"{"builtInDisplayID":1,"restored":true}"#).unwrap();
        assert!(!old.automation_suspended);
        let suspended: Recovery = serde_json::from_str(
            r#"{"builtInDisplayID":1,"restored":true,"automationSuspended":true}"#,
        ).unwrap();
        assert!(suspended.automation_suspended);
    }

    #[test]
    fn independent_watchdog_preserves_sleep_and_resets_on_reconnect() {
        let now = Instant::now();
        let mut guard = UnplugWatchdog::default();
        let asleep = SafetySnapshot { external_state: 2, built_in_usable: false };
        let ready = SafetySnapshot { external_state: 0, built_in_usable: false };
        assert!(!guard.poll(Some(asleep), now));
        assert!(!guard.poll(Some(asleep), now + Duration::from_secs(30)));
        assert!(!guard.poll(Some(SafetySnapshot::unavailable()), now + Duration::from_secs(31)));
        assert!(!guard.poll(Some(ready), now + Duration::from_secs(32)));
        assert!(!guard.poll(None, now + Duration::from_secs(40)));
    }

    #[test]
    fn restore_timeout_releases_the_command_for_another_emergency_request() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let started = Instant::now();
        let error = run_with_timeout(&mut command, Duration::from_millis(50)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));

        let output = run_with_timeout(&mut Command::new("/usr/bin/true"), Duration::from_secs(1)).unwrap();
        assert!(output.status.success());
    }

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
