mod display;
mod icon;

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
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
const POLL: Duration = Duration::from_millis(250);

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
    state: Mutex<Option<State>>,
    /// Main display refresh rate shown next to the icon; outer None until first read.
    refresh_rate: Mutex<Option<Option<u32>>>,
    /// When the last toggle was requested, to log how long the display took to follow.
    requested: Mutex<Option<Instant>>,
    tray: Mutex<Option<TrayIcon>>,
    /// Set when the popover hides on blur, so the same tray click doesn't reopen it.
    hidden_at: Mutex<Option<Instant>>,
}

/// Whether the supervisor should be running; restored at launch (e.g. after a reboot).
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
            let app = app.clone();
            thread::spawn(move || {
                match child.wait() {
                    Ok(status) => log::info!("supervisor {pid} exited: {status}"),
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
    if controller.busy.swap(true, Ordering::SeqCst) {
        log::debug!("toggle ignored: previous toggle still running");
        return;
    }
    *controller.requested.lock().unwrap() = Some(Instant::now());
    let app = app.clone();
    thread::spawn(move || {
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

fn refresh(app: &AppHandle) {
    refresh_rate(app);
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
        .with_shortcuts([SHORTCUT])
        .expect("invalid shortcut")
        .with_handler(|app, shortcut, event| {
            if event.state == ShortcutState::Pressed {
                log::debug!("shortcut {shortcut:?}");
                toggle(app);
            }
        })
        .build();

    tauri::Builder::default()
        // Must be first: a second copy would start a second supervisor and tray icon.
        .plugin(tauri_plugin_single_instance::init(|_, _, _| log::info!("another instance tried to start")))
        .plugin(log_plugin)
        .plugin(shortcut_plugin)
        .manage(Controller::default())
        .invoke_handler(tauri::generate_handler![get_state, toggle_display, hide])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle();
            log::info!("displayctl menu bar app starting; cli {}", display::binary().display());

            let (state, supervising) = current_state();
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let tray = TrayIconBuilder::with_id("main")
                .icon(Image::new_owned(icon::rgba(state), icon::SIZE as u32, icon::SIZE as u32))
                .icon_as_template(true)
                .menu(&Menu::with_items(app, &[&quit])?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id() == "quit" {
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

            if desired_off_path(handle).exists() && !supervising {
                log::info!("restoring display-off from last session");
                start_supervisor(handle);
            } else if supervising {
                set_desired_off(handle, true);
            }

            // `pkill -USR1 displayctl-menubar` toggles too, for scripts and testing.
            let signals = handle.clone();
            let mut usr1 = signal_hook::iterator::Signals::new([signal_hook::consts::SIGUSR1])?;
            thread::spawn(move || {
                for _ in usr1.forever() {
                    log::debug!("SIGUSR1");
                    toggle(&signals);
                }
            });

            let poller = handle.clone();
            thread::spawn(move || loop {
                thread::sleep(POLL);
                refresh(&poller);
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
