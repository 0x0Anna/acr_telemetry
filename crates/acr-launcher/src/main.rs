//! Slint GUI shell for `acr_telemetry`'s recorder/export/status tools.
//! Unit A of docs/plans/acr-launcher-v1.md: this pass wires up the
//! window shell (Status/Record/Export tabs), a live "is ACC/AC Rally
//! running" poll for the Status tab, and `process.rs`'s reusable
//! child-process helpers for the next two units (recorder panel, export
//! panel) to build on.
//!
//! Conventions (matching the sibling `shakedown-engineer` repo's
//! `sde-app`): one `Rc<RefCell<AppState>>` shared across callbacks, a
//! `window.as_weak()` captured per closure and `upgrade()`d inside it
//! (so a closed window doesn't keep background work alive), and one
//! `window.on_xxx(move |...| { ... })` registration block per callback.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acc_shared_memory_rs::{ACCError, ACCSharedMemory};
use slint::ComponentHandle;

pub mod process;

slint::include_modules!();

/// Everything that needs to survive between callbacks. Deliberately
/// minimal for this unit — the recorder/export panels will grow this
/// with their own config/child-process state (see `process.rs`'s
/// doc comment for the shape they're expected to reuse).
#[derive(Default)]
struct AppState {
    /// Set once `acr_recorder::config::load_config()` is wired up by the
    /// recorder panel unit; left unused here to keep this skeleton
    /// buildable without pulling in the full config surface yet.
    _config: Option<acr_recorder::config::Config>,
}

/// Poll `ACCSharedMemory::new()` once a second on a background thread
/// and push "Connected"/"not detected" transitions into `status-text`
/// via `slint::invoke_from_event_loop` — the same crash-safe call the
/// backend fix in `acr_recorder::acc_wait` uses, just polled instead of
/// blocked-on, since the launcher window needs to keep running either
/// way. Deliberately independent of any recorder/export subprocess: the
/// Status tab should reflect whether the game itself is up, not whether
/// a recording happens to be active.
fn spawn_status_poll(window: &AppWindow, running: Arc<AtomicBool>) {
    let window_weak = window.as_weak();

    std::thread::spawn(move || {
        let mut last_connected: Option<bool> = None;

        while running.load(Ordering::Relaxed) {
            let connected = match ACCSharedMemory::new() {
                Ok(_) => Some(true),
                Err(ACCError::SharedMemoryNotAvailable) => Some(false),
                Err(e) => {
                    eprintln!("acc_shared_memory poll error: {e}");
                    None
                }
            };

            if let Some(connected) = connected {
                if last_connected != Some(connected) {
                    last_connected = Some(connected);
                    let window_weak = window_weak.clone();
                    let text = if connected {
                        "ACC/AC Rally: connected".to_string()
                    } else {
                        "ACC/AC Rally: not detected".to_string()
                    };
                    // Slint window handles aren't Send, so the update has
                    // to be posted onto the UI thread's event loop rather
                    // than touched directly from this background thread.
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = window_weak.upgrade() {
                            window.set_status_text(text.into());
                        }
                    });
                }
            }

            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let _state: Rc<RefCell<AppState>> = Rc::new(RefCell::new(AppState::default()));

    let poll_running = Arc::new(AtomicBool::new(true));
    spawn_status_poll(&window, poll_running.clone());

    window.run()?;

    // Let the poll thread's `while running` loop notice and exit rather
    // than leaking it past window close (it'll wake up within a second).
    poll_running.store(false, Ordering::Relaxed);

    Ok(())
}
