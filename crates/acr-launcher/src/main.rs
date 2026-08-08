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

#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acc_shared_memory_rs::{ACCError, ACCSharedMemory};
use slint::ComponentHandle;

pub mod analysis_export_panel;
pub mod export_panel;
pub mod grip_estimator_panel;
pub mod hotkeys;
pub mod launcher_config;
pub mod plot_recording_panel;
pub mod process;
pub mod recorder_panel;
pub mod telemetry_bridge_panel;
pub mod track_match_panel;

slint::include_modules!();

use export_panel::ExportInput;

/// Everything that needs to survive between callbacks. Shared across the
/// Status/Record/Export panels via `Rc<RefCell<AppState>>` — see
/// `recorder_panel.rs`'s `init` for how the Record tab reads/writes
/// `config`.
pub(crate) struct AppState {
    /// Loaded once at startup via `acr_recorder::config::load_config()`,
    /// edited in place by the Record tab, and written back out with
    /// `toml::to_string_pretty` on Save. The export panel also reads
    /// `raw_output_dir`/`sqlite_db_path` defaults from it.
    pub(crate) config: acr_recorder::config::Config,
    /// Currently selected export input (file/dir/raw-dir), set by the
    /// Export tab's pick buttons; consumed by `export_panel::run_export`.
    pub(crate) export_input: Option<ExportInput>,
    /// Reference track file(s) or a single directory, picked by the
    /// Track Match tab's pick buttons; joined into `acr_track_match`'s
    /// `--refs path[,path...]` argument by `track_match_panel::refs_arg`.
    pub(crate) track_match_refs: Vec<std::path::PathBuf>,
    /// Offline mode's `.rkyv` input file, picked by the Track Match tab.
    pub(crate) track_match_input: Option<std::path::PathBuf>,
    /// The physics `.rkyv` input picked by the Plot Recording tab.
    pub(crate) plot_recording_input: Option<std::path::PathBuf>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            config: acr_recorder::config::load_config(),
            export_input: None,
            track_match_refs: Vec::new(),
            track_match_input: None,
            plot_recording_input: None,
        }
    }
}

/// AC Rally's game process image name — checked via `tasklist` (see
/// `process::is_process_running`) to drive the "Launch AC Rally" button
/// independent of the ACC-shared-memory poll below. Shared-memory presence
/// alone isn't a reliable "is the game running" signal: overlay tools like
/// SimHub keep the same ACC-shaped shared memory segments alive on their
/// own even when the game itself isn't running.
const ACR_PROCESS_NAME: &str = "acr.exe";

/// Poll `ACCSharedMemory::new()` once a second on a background thread and
/// push "Connected"/"not detected" transitions into `status-text` via
/// `slint::invoke_from_event_loop` — the same crash-safe call the backend
/// fix in `acr_recorder::acc_wait` uses, just polled instead of
/// blocked-on, since the launcher window needs to keep running either
/// way. Deliberately independent of any recorder/export subprocess: the
/// Status tab should reflect whether the game itself is up, not whether a
/// recording happens to be active.
///
/// Also polls `ACR_PROCESS_NAME` via `tasklist` on the same tick and pushes
/// its own transitions into `acr-process-running` — kept separate from the
/// shared-memory `connected` state above (see `ACR_PROCESS_NAME`'s doc
/// comment for why they can disagree).
fn spawn_status_poll(window: &AppWindow, running: Arc<AtomicBool>) {
    let window_weak = window.as_weak();

    std::thread::spawn(move || {
        let mut last_connected: Option<bool> = None;
        let mut last_process_running: Option<bool> = None;

        while running.load(Ordering::Relaxed) {
            let connected = match ACCSharedMemory::new() {
                Ok(_) => Some(true),
                Err(ACCError::SharedMemoryNotAvailable) => Some(false),
                Err(e) => {
                    crate::process::log_err(format!("acc_shared_memory poll error: {e}"));
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
                            window.set_acr_running(connected);
                        }
                    });
                }
            }

            let process_running = process::is_process_running(ACR_PROCESS_NAME);
            if last_process_running != Some(process_running) {
                last_process_running = Some(process_running);
                let window_weak = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = window_weak.upgrade() {
                        window.set_acr_process_running(process_running);
                    }
                });
            }

            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// AC Rally's Steam App ID — used to build the `steam://run/<id>` URI that
/// `on_launch_acr` hands off to `explorer` (Steam registers this protocol
/// on install; `explorer` invoking it is the same mechanism a desktop
/// shortcut or Start Menu entry uses).
const ACR_STEAM_APP_ID: &str = "3917090";

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let state: Rc<RefCell<AppState>> = Rc::new(RefCell::new(AppState::default()));

    window.on_launch_acr(|| {
        let _ = std::process::Command::new("explorer")
            .arg(format!("steam://run/{ACR_STEAM_APP_ID}"))
            .spawn();
    });

    export_panel::init(&window, state.clone());
    recorder_panel::init(&window, state.clone());
    hotkeys::init(&window, state.clone());
    track_match_panel::init(&window, state.clone());
    plot_recording_panel::init(&window, state.clone());
    grip_estimator_panel::init(&window, state.clone());
    telemetry_bridge_panel::init(&window, state.clone());
    analysis_export_panel::init(&window, state.clone());

    let poll_running = Arc::new(AtomicBool::new(true));
    spawn_status_poll(&window, poll_running.clone());

    {
        let state = state.clone();
        let window_weak = window.as_weak();
        window.window().on_close_requested(move || {
            if let Some(window) = window_weak.upgrade() {
                stop_running_children(&window, &state);
            }
            slint::CloseRequestResponse::HideWindow
        });
    }

    window.run()?;

    // Let the poll thread's `while running` loop notice and exit rather
    // than leaking it past window close (it'll wake up within a second).
    poll_running.store(false, Ordering::Relaxed);

    Ok(())
}

/// Write the stop file for every long-running child that's still marked
/// `running` when the window is closing, so closing the launcher doesn't
/// orphan a recording/bridge/live-match/analysis-export-serve process —
/// each of those is spawned hidden with no shared console for Ctrl+C to
/// reach, so a stop file is the only way anything but the launcher itself
/// can ask them to exit. Best-effort: this fires once on close, doesn't
/// wait for the children to actually finish exiting (they poll for the
/// stop file on their own schedule, up to ~1s), and any write failure is
/// silently ignored since there's no UI left to surface it to.
fn stop_running_children(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    if window.get_recorder_running() {
        let cfg = state.borrow().config.clone();
        let _ = std::fs::write(acr_recorder::config::resolve_stop_file_path(&cfg.recorder), b"");
    }
    if window.get_track_match_running() {
        let _ = std::fs::write(acr_recorder::track_match_app::stop_file_path(), b"");
    }
    if window.get_telemetry_bridge_running() {
        let _ = std::fs::write(telemetry_bridge_panel::stop_file_path(), b"");
    }
    if window.get_analysis_export_serve_running() {
        let _ = std::fs::write(analysis_export_panel::serve_stop_file_path(), b"");
    }
}
