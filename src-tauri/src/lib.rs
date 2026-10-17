mod caffeine;
mod display;
mod icon;

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, Rect, WindowEvent,
};
use tauri_plugin_global_shortcut::ShortcutState;

const SHORTCUT: &str = "ctrl+shift+d";
const EMERGENCY_SHORTCUT: &str = "ctrl+alt+shift+d";
const EMERGENCY_ALIAS: &str = "alt+shift+d";
const POLL: Duration = Duration::from_secs(1);
const REFRESH_RATE_POLL: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Built-in display on, no supervisor.
    On,
    /// Supervisor running, built-in display still on (no external monitor ready yet).
    Armed,
    /// Built-in display off.
    Off,
}

fn current_state() -> (State, bool) {
    let supervising = display::supervisor_pid().is_some();
    let state = match (display::built_in_online(), supervising) {
        (false, _) => State::Off,
        (true, true) => State::Armed,
        (true, false) => State::On,
    };
    (state, supervising)
}

#[derive(Default)]
struct Controller {
    busy: AtomicBool,
    emergency_pending: AtomicBool,
    stop_epoch: AtomicU64,
    command: Mutex<()>,
    state: Mutex<Option<State>>,
    /// Main display refresh rate shown next to the icon; outer None until first read.
    refresh_rate: Mutex<Option<Option<u32>>>,
    /// When the last toggle was requested, to log how long the display took to follow.
    requested: Mutex<Option<Instant>>,
    tray: Mutex<Option<TrayIcon>>,
    /// Set when the popover hides on blur, so the same tray click doesn't reopen it.
    hidden_at: Mutex<Option<Instant>>,
}

/// Whether to resume the supervisor when the app is manually relaunched.
fn desired_off_path(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().expect("no app data dir").join("display-off")
}

fn set_desired_off(app: &AppHandle, off: bool) {
    let path = desired_off_path(app);
    let result = if off {
        fs::create_dir_all(path.parent().unwrap()).and_then(|_| fs::write(&path, b""))
    } else if path.exists() {
        fs::remove_file(&path)
    } else {
        Ok(())
    };
    if let Err(error) = result {
        log::error!("could not save desired state: {error}");
    }
}

fn start_supervisor(app: &AppHandle) {
    log::info!("starting supervisor: {} off", display::binary().display());
    match display::start_supervisor() {
        Ok(mut child) => {
            let pid = child.id();
            log::debug!("supervisor pid {pid}");
            // Serialize emergency restore after the child has published its PID.
            let deadline = Instant::now() + Duration::from_secs(5);
            while display::supervisor_pid() != Some(pid as i32) {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                if Instant::now() >= deadline {
                    log::error!("supervisor did not publish recovery state; stopping it");
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = display::restore();
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            let app = app.clone();
            thread::spawn(move || {
                match child.wait() {
                    Ok(status) => {
                        log::info!("supervisor {pid} exited: {status}");
                        if status.code() == Some(75) {
                            let controller = app.state::<Controller>();
                            let _command = controller.command.lock().unwrap();
                            // Do not cancel a newer, explicit request to start.
                            if display::supervisor_pid().is_none() {
                                set_desired_off(&app, false);
                            }
                        }
                    }
                    Err(error) => log::error!("waiting for supervisor {pid}: {error}"),
                }
                refresh(&app);
            });
        }
        Err(error) => log::error!("could not start supervisor: {error}"),
    }
}

fn toggle(app: &AppHandle) {
    let controller = app.state::<Controller>();
    let stop_epoch = controller.stop_epoch.load(Ordering::SeqCst);
    if controller.emergency_pending.load(Ordering::SeqCst) || controller.busy.swap(true, Ordering::SeqCst) {
        log::debug!("toggle ignored: previous toggle still running");
        return;
    }
    *controller.requested.lock().unwrap() = Some(Instant::now());
    let app = app.clone();
    thread::spawn(move || {
        let controller = app.state::<Controller>();
        let _command = controller.command.lock().unwrap();
        if controller.emergency_pending.load(Ordering::SeqCst)
            || controller.stop_epoch.load(Ordering::SeqCst) != stop_epoch
        {
            controller.busy.store(false, Ordering::SeqCst);
            return;
        }
        let started = Instant::now();
        if let Some(pid) = display::supervisor_pid() {
            log::info!("toggle: restoring built-in display (supervisor {pid})");
            set_desired_off(&app, false);
            match display::restore() {
                Ok(output) => {
                    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
                    log::info!("displayctl on: {} {}", output.status, text.trim());
                }
                Err(error) => log::error!("could not run displayctl on: {error}"),
            }
        } else {
            log::info!("toggle: turning built-in display off");
            set_desired_off(&app, true);
            start_supervisor(&app);
        }
        log::debug!("toggle command returned after {:?}", started.elapsed());
        app.state::<Controller>().busy.store(false, Ordering::SeqCst);
        refresh(&app);
    });
}

/// Always restore; repeated presses must never re-enable external-only mode.
fn emergency_off(app: &AppHandle) {
    let controller = app.state::<Controller>();
    if controller.emergency_pending.swap(true, Ordering::SeqCst) {
        return;
    }
    controller.stop_epoch.fetch_add(1, Ordering::SeqCst);
    let app = app.clone();
    thread::spawn(move || {
        let controller = app.state::<Controller>();
        let _command = controller.command.lock().unwrap();
        set_desired_off(&app, false);
        log::warn!("emergency off: stopping automation and restoring built-in display");
        match display::restore() {
            Ok(output) => log::info!(
                "emergency restore: {} {} {}", output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => log::error!("could not run emergency restore: {error}"),
        }
        refresh(&app);
        controller.emergency_pending.store(false, Ordering::SeqCst);
    });
}

fn recover_unplug(app: &AppHandle) {
    let controller = app.state::<Controller>();
    let stop_epoch = controller.stop_epoch.load(Ordering::SeqCst);
    let _command = controller.command.lock().unwrap();
    if controller.emergency_pending.load(Ordering::SeqCst)
        || controller.stop_epoch.load(Ordering::SeqCst) != stop_epoch
    {
        return;
    }
    // Recheck after waiting for any toggle; a reconnect or a completed restore
    // must cancel this request before it changes the display configuration.
    let snapshot = match display::safety_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log::warn!("safety probe unavailable before recovery: {error}");
            Some(display::SafetySnapshot::after_probe_failure())
        }
    };
    if !snapshot.is_some_and(display::SafetySnapshot::needs_recovery) {
        return;
    }
    let resume = desired_off_path(app).exists()
        && snapshot.is_some_and(display::SafetySnapshot::can_resume_after_recovery)
        && !display::automation_suspended();
    if !resume {
        set_desired_off(app, false);
    }
    log::warn!("independent unplug watchdog: restoring; resume automatic switching after confirmed unplug: {resume}");
    match display::restore() {
        Ok(output) if output.status.success() => {
            log::info!("unplug recovery: {}", String::from_utf8_lossy(&output.stdout).trim());
            if resume && !display::automation_suspended()
                && !controller.emergency_pending.load(Ordering::SeqCst)
                && controller.stop_epoch.load(Ordering::SeqCst) == stop_epoch
            {
                // Continue supervising reconnects after replacing a stalled worker.
                start_supervisor(app);
            }
        }
        Ok(output) => log::error!("unplug recovery failed: {} {}", output.status, String::from_utf8_lossy(&output.stderr).trim()),
        Err(error) => log::error!("unplug recovery failed: {error}"),
    }
    refresh(app);
}

fn refresh(app: &AppHandle) {
    let (state, _) = current_state();
    let controller = app.state::<Controller>();
    let mut previous = controller.state.lock().unwrap();
    if *previous == Some(state) {
        return;
    }
    match controller.requested.lock().unwrap().take() {
        Some(at) => log::info!("state: {state:?} ({:?} after toggle)", at.elapsed()),
        None => log::info!("state: {state:?}"),
    }
    *previous = Some(state);
    if let Some(tray) = controller.tray.lock().unwrap().as_ref() {
        let _ = tray.set_icon(Some(Image::new_owned(icon::rgba(state), icon::SIZE as u32, icon::SIZE as u32)));
        let _ = tray.set_icon_as_template(true);
    }
    let _ = app.emit("state", state);
}

fn refresh_rate(app: &AppHandle) {
    let rate = display::main_refresh_rate();
    let controller = app.state::<Controller>();
    let mut previous = controller.refresh_rate.lock().unwrap();
    if *previous == Some(rate) {
        return;
    }
    *previous = Some(rate);
    if let Some(tray) = controller.tray.lock().unwrap().as_ref() {
        let _ = tray.set_title(rate.map(|hz| hz.to_string()));
    }
    log::info!("main display refresh rate: {rate:?}");
}

fn toggle_popover(app: &AppHandle, rect: Rect) {
    let Some(window) = app.get_webview_window("popover") else { return };
    let controller = app.state::<Controller>();
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    if controller.hidden_at.lock().unwrap().is_some_and(|at| at.elapsed() < Duration::from_millis(300)) {
        return;
    }
    let scale = window.scale_factor().unwrap_or(2.0);
    let position = rect.position.to_physical::<f64>(scale);
    let size = rect.size.to_physical::<f64>(scale);
    if let Ok(window_size) = window.outer_size() {
        let x = position.x + size.width / 2.0 - window_size.width as f64 / 2.0;
        let y = position.y + size.height + 6.0 * scale;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    let _ = window.set_focus();
}

#[tauri::command]
fn get_state() -> State {
    log::debug!("popover requested state");
    current_state().0
}

#[tauri::command]
fn toggle_display(app: AppHandle) {
    toggle(&app);
}

#[tauri::command]
fn hide(app: AppHandle) {
    if let Some(window) = app.get_webview_window("popover") {
        let _ = window.hide();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let log_plugin = tauri_plugin_log::Builder::new()
        .level(log::LevelFilter::Debug)
        .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
        .level_for("tao", log::LevelFilter::Info)
        .targets([
            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                path: display::log_dir(),
                file_name: Some("app".into()),
            }),
        ])
        .build();

    let shortcut_plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_shortcuts([SHORTCUT, EMERGENCY_SHORTCUT, EMERGENCY_ALIAS])
        .expect("invalid shortcut")
        .with_handler(|app, shortcut, event| {
            if event.state == ShortcutState::Pressed {
                log::debug!("shortcut {shortcut:?}");
                if shortcut == &SHORTCUT.parse::<tauri_plugin_global_shortcut::Shortcut>().unwrap() {
                    toggle(app);
                } else {
                    emergency_off(app);
                }
            }
        })
        .build();

    tauri::Builder::default()
        // Must be first: a second copy would start a second supervisor and tray icon.
        .plugin(tauri_plugin_single_instance::init(|_, _, _| log::info!("another instance tried to start")))
        .plugin(log_plugin)
        .plugin(shortcut_plugin)
        .manage(Controller::default())
        .manage(caffeine::Caffeine::default())
        .invoke_handler(tauri::generate_handler![get_state, toggle_display, hide, caffeine::get_caffeine, caffeine::set_caffeine])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle();
            log::info!("displayctl menu bar app starting; cli {}", display::binary().display());

            let (state, supervising) = current_state();
            log::info!("emergency off shortcuts registered: {EMERGENCY_SHORTCUT}, {EMERGENCY_ALIAS}");
            let emergency = MenuItem::with_id(app, "emergency", "Emergency Off — Restore Built-in Display", true, Some(EMERGENCY_SHORTCUT))?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let tray = TrayIconBuilder::with_id("main")
                .icon(Image::new_owned(icon::rgba(state), icon::SIZE as u32, icon::SIZE as u32))
                .icon_as_template(true)
                .menu(&Menu::with_items(app, &[&emergency, &quit])?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id() == "emergency" {
                        emergency_off(app);
                    } else if event.id() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = event {
                        toggle_popover(tray.app_handle(), rect);
                    }
                })
                .build(app)?;
            *app.state::<Controller>().tray.lock().unwrap() = Some(tray);
            *app.state::<Controller>().state.lock().unwrap() = Some(state);
            log::info!("state: {state:?}");
            refresh_rate(handle);

            if display::automation_suspended() {
                set_desired_off(handle, false);
                log::warn!("automatic switching remains suspended after an unavailable display probe");
            } else if desired_off_path(handle).exists() && !supervising {
                log::info!("restoring display-off from last session");
                let controller = app.state::<Controller>();
                let _command = controller.command.lock().unwrap();
                if !controller.emergency_pending.load(Ordering::SeqCst) {
                    start_supervisor(handle);
                }
            } else if supervising {
                set_desired_off(handle, true);
            }

            // `pkill -USR1 displayctl-menubar` toggles too, for scripts and testing.
            let signals = handle.clone();
            let mut usr1 = signal_hook::iterator::Signals::new([
                signal_hook::consts::SIGUSR1, signal_hook::consts::SIGUSR2,
            ])?;
            thread::spawn(move || {
                for signal in usr1.forever() {
                    if signal == signal_hook::consts::SIGUSR2 {
                        emergency_off(&signals);
                    } else {
                        toggle(&signals);
                    }
                }
            });

            let poller = handle.clone();
            thread::spawn(move || loop {
                thread::sleep(POLL);
                refresh(&poller);
            });

            let poller = handle.clone();
            thread::spawn(move || loop {
                thread::sleep(REFRESH_RATE_POLL);
                refresh_rate(&poller);
            });

            // Diagnostic: when the DisplayPort hot-plug line changes, relative to
            // WindowServer's own hot-plug events, to measure unplug latency.
            thread::spawn(|| {
                let mut hot_plugged = display::display_port_hot_plugged();
                log::info!("display port hot-plug: {hot_plugged}");
                loop {
                    thread::sleep(Duration::from_millis(100));
                    let now = display::display_port_hot_plugged();
                    if now != hot_plugged {
                        log::info!("display port hot-plug: {hot_plugged} -> {now} at {:?}", std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default());
                        hot_plugged = now;
                    }
                }
            });

            let safety = handle.clone();
            thread::spawn(move || {
                let mut watchdog = display::UnplugWatchdog::default();
                let mut last_snapshot = None;
                let mut last_log = Instant::now();
                log::info!("independent unplug watchdog started");
                loop {
                    thread::sleep(Duration::from_secs(1));
                    if display::automation_suspended() && desired_off_path(&safety).exists() {
                        let controller = safety.state::<Controller>();
                        let _command = controller.command.lock().unwrap();
                        // Recheck under the same lock as an explicit toggle.
                        if display::automation_suspended() {
                            set_desired_off(&safety, false);
                        }
                    }
                    let snapshot = match display::safety_snapshot() {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            log::warn!("independent safety probe failed: {error}");
                            Some(display::SafetySnapshot::after_probe_failure())
                        }
                    };
                    if last_snapshot != Some(snapshot)
                        || (snapshot.is_some() && last_log.elapsed() >= Duration::from_secs(30))
                    {
                        log::info!("independent safety state: {snapshot:?}");
                        last_snapshot = Some(snapshot);
                        last_log = Instant::now();
                    }
                    if watchdog.poll(snapshot, Instant::now()) {
                        recover_unplug(&safety);
                    }
                }
            });

            if let Some(window) = app.get_webview_window("popover") {
                let popover = window.clone();
                let handle = handle.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::Focused(false) = event {
                        let _ = popover.hide();
                        *handle.state::<Controller>().hidden_at.lock().unwrap() = Some(Instant::now());
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running displayctl");
}
