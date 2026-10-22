//! "Keep the computer awake": stops the computer going to sleep when idle while
//! Warden runs, so reviews, triage and fixes carry on when nobody is at it. The
//! screen can still turn off and lock; only sleep is held off, and only while the
//! setting is on and Warden is running.

use std::sync::Mutex;

static GUARD: Mutex<Option<Guard>> = Mutex::new(None);

enum Guard {
    /// macOS: `caffeinate -i -w <warden>`, which also ends if Warden does.
    #[cfg(target_os = "macos")]
    Caffeinate(std::process::Child),
    /// Windows: a thread holding `ES_SYSTEM_REQUIRED`; dropping the sender ends it.
    #[cfg(windows)]
    Thread(#[allow(dead_code)] std::sync::mpsc::Sender<()>),
}

impl Drop for Guard {
    fn drop(&mut self) {
        match self {
            #[cfg(target_os = "macos")]
            Guard::Caffeinate(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(windows)]
            Guard::Thread(_) => {}
        }
    }
}

/// Holds off sleep (`on`) or lets the computer sleep again. Safe to call repeatedly.
pub fn keep_awake(on: bool) {
    let mut guard = GUARD.lock().unwrap();
    if !on {
        *guard = None;
        return;
    }
    if guard.is_some() {
        return;
    }
    *guard = start();
}

#[cfg(target_os = "macos")]
fn start() -> Option<Guard> {
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Guard::Caffeinate)
        .map_err(|e| eprintln!("warden: could not keep the computer awake: {e}"))
        .ok()
}

#[cfg(windows)]
fn start() -> Option<Guard> {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        // The request belongs to this thread, so it waits here until told to stop.
        unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
        let _ = rx.recv();
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    });
    Some(Guard::Thread(tx))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn start() -> Option<Guard> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn caffeinated() -> bool {
        let me = std::process::id().to_string();
        let out = std::process::Command::new("pgrep").args(["-f", &format!("caffeinate -i -w {me}")]).output().unwrap();
        !out.stdout.is_empty()
    }

    #[test]
    fn sleep_is_held_off_only_while_asked() {
        keep_awake(true);
        keep_awake(true);
        assert!(caffeinated());
        keep_awake(false);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!caffeinated());
    }
}
