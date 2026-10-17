use std::{
    process::{Child, Command},
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Caffeine(Mutex<Session>);

struct Session {
    minutes: u32,
    child: Option<Child>,
    deadline: Option<Instant>,
}

impl Default for Session {
    fn default() -> Self {
        Self { minutes: 120, child: None, deadline: None }
    }
}

impl Session {
    fn stop(&mut self) -> Result<(), String> {
        if let Some(child) = self.child.as_mut() {
            // Reap an already-finished process before attempting to signal it.
            if child.try_wait().map_err(|e| e.to_string())?.is_none() {
                child.kill().map_err(|e| e.to_string())?;
                child.wait().map_err(|e| e.to_string())?;
            }
        }
        self.child = None;
        self.deadline = None;
        Ok(())
    }

    fn status(&mut self) -> Result<Status, String> {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                self.child = None;
                self.deadline = None;
            }
        }
        Ok(Status {
            minutes: self.minutes,
            active: self.child.is_some(),
            remaining_seconds: self.deadline.map(|d| d.saturating_duration_since(Instant::now()).as_secs()),
        })
    }
}

#[derive(serde::Serialize)]
pub struct Status {
    minutes: u32,
    active: bool,
    remaining_seconds: Option<u64>,
}

#[tauri::command]
pub fn get_caffeine(state: tauri::State<'_, Caffeine>) -> Result<Status, String> {
    state.0.lock().unwrap().status()
}

#[tauri::command]
pub fn set_caffeine(state: tauri::State<'_, Caffeine>, active: bool, minutes: u32) -> Result<Status, String> {
    if !(30..=1440).contains(&minutes) || minutes % 30 != 0 {
        return Err("Choose 30 minutes to 24 hours, in 30-minute steps.".into());
    }
    let mut session = state.0.lock().unwrap();
    session.stop()?;
    session.minutes = minutes;
    if active {
        let seconds = minutes * 60;
        let child = Command::new("/usr/bin/caffeinate")
            // Keep the system and display awake; release on timeout or app exit.
            .args(["-di", "-t", &seconds.to_string(), "-w", &std::process::id().to_string()])
            .spawn()
            .map_err(|e| format!("Could not start caffeinate: {e}"))?;
        session.child = Some(child);
        session.deadline = Some(Instant::now() + Duration::from_secs(seconds.into()));
    }
    let status = session.status()?;
    log::info!("caffeinate: active={}, duration={} minutes", status.active, status.minutes);
    Ok(status)
}
