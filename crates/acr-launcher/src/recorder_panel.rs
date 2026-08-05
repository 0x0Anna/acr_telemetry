//! Record tab (unit C of docs/plans/acr-launcher-v1.md): load/edit/save
//! `acr_recorder.toml`, and start/stop a recording by spawning either
//! `acr_recorder.exe` (rkyv) or `acr_motec.exe` (direct-to-MoTeC, "no
//! rkyv") depending on the "Record directly to MoTeC" toggle — mirroring
//! the existing `acr_recorder --motec` / `acr_motec` binary duality.
//!
//! Follows the same shape described for the (not-yet-built, at the time
//! this unit landed) export panel: `pub(crate) fn init(window, state)`
//! wires up callbacks, a background thread reads `process::stream_output`
//! output and posts updates back onto the UI thread via
//! `slint::invoke_from_event_loop`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

use slint::ComponentHandle;

use crate::process::{self, ChildOutput};
use crate::{AppState, AppWindow};

pub(crate) fn init(window: &AppWindow, state: Rc<RefCell<AppState>>) {
    sync_config_to_ui(window, &state.borrow().config);

    {
        let state = state.clone();
        let window_weak = window.as_weak();
        window.on_recorder_save(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let mut app_state = state.borrow_mut();
            apply_ui_to_config(&window, &mut app_state.config);
            match save_config(&app_state.config) {
                Ok(path) => window
                    .set_recorder_status_text(format!("Saved to {}", path.display()).into()),
                Err(e) => window.set_recorder_status_text(format!("Save failed: {e}").into()),
            }
        });
    }

    {
        let state = state.clone();
        let window_weak = window.as_weak();
        window.on_recorder_start(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };

            if window.get_recorder_running() {
                // Already have a child streaming; ignore a double-click.
                return;
            }

            {
                let mut app_state = state.borrow_mut();
                apply_ui_to_config(&window, &mut app_state.config);
                if let Err(e) = save_config(&app_state.config) {
                    window.set_recorder_status_text(format!("Save failed: {e}").into());
                    return;
                }
            }

            start_recording(&window);
        });
    }

    {
        let state = state.clone();
        let window_weak = window.as_weak();
        window.on_recorder_stop(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let cfg = state.borrow().config.clone();
            let stop_path = acr_recorder::config::resolve_stop_file_path(&cfg.recorder);
            match std::fs::write(&stop_path, b"") {
                Ok(()) => {
                    window.set_recorder_status_pill("Stopping…".into());
                    append_log(&window, &format!("Wrote stop file: {}", stop_path.display()));
                }
                Err(e) => {
                    window.set_recorder_status_text(
                        format!("Failed to write stop file: {e}").into(),
                    );
                }
            }
        });
    }
}

/// Push `cfg.recorder`'s fields into the Record tab's editable properties.
/// Called once at startup (before any callback is wired up) to pre-fill
/// the form from the on-disk `acr_recorder.toml`.
fn sync_config_to_ui(window: &AppWindow, cfg: &acr_recorder::config::Config) {
    let r = &cfg.recorder;
    window.set_recorder_raw_output_dir(r.raw_output_dir.clone().into());
    window.set_recorder_notes_dir(r.notes_dir.clone().unwrap_or_default().into());
    window.set_recorder_record_graphics(r.record_graphics);
    window.set_recorder_ring_mode(r.ring_mode);
    window.set_recorder_ring_slots(r.ring_slots.to_string().into());
    window.set_recorder_ring_prefix(r.ring_prefix.clone().into());
}

/// Read the Record tab's editable properties back into `cfg.recorder`.
/// Leaves fields the panel doesn't expose (distance-reset tuning, etc.)
/// untouched.
fn apply_ui_to_config(window: &AppWindow, cfg: &mut acr_recorder::config::Config) {
    let r = &mut cfg.recorder;
    r.raw_output_dir = window.get_recorder_raw_output_dir().to_string();

    let notes = window.get_recorder_notes_dir().to_string();
    r.notes_dir = if notes.trim().is_empty() {
        None
    } else {
        Some(notes)
    };

    r.record_graphics = window.get_recorder_record_graphics();
    r.ring_mode = window.get_recorder_ring_mode();

    if let Ok(slots) = window.get_recorder_ring_slots().parse::<usize>() {
        r.ring_slots = slots.max(2);
    }

    let prefix = window.get_recorder_ring_prefix().to_string();
    if !prefix.trim().is_empty() {
        r.ring_prefix = prefix;
    }
}

/// Where `acr_recorder.toml` is written back to: next to the launcher's
/// own executable, matching `load_config`'s first (and, per the README's
/// existing convention of keeping all `acr_*.exe` together, expected)
/// search path.
fn config_file_path() -> PathBuf {
    acr_recorder::config::base_dir()
        .map(|dir| dir.join("acr_recorder.toml"))
        .unwrap_or_else(|| PathBuf::from("acr_recorder.toml"))
}

/// Serialize the full `Config` (both `[recorder]` and `[export]`) back out
/// with `toml::to_string_pretty`. Known trade-off (documented in
/// docs/plans/acr-launcher-v1.md): this does not preserve comments or
/// formatting in a hand-edited `acr_recorder.toml` — acceptable for v1.
fn save_config(cfg: &acr_recorder::config::Config) -> std::io::Result<PathBuf> {
    let path = config_file_path();
    let text = toml::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, &text)?;
    Ok(path)
}

/// Spawn the chosen binary (`acr_motec.exe` if "Record directly to
/// MoTeC" is checked, `acr_recorder.exe` otherwise) and start tailing its
/// output into the log panel / status pill.
fn start_recording(window: &AppWindow) {
    let motec_mode = window.get_recorder_motec_mode();
    let binary_name = if motec_mode {
        "acr_motec.exe"
    } else {
        "acr_recorder.exe"
    };
    let binary_path = process::resolve_binary(binary_name);

    let child = match process::spawn_hidden(&binary_path, &[]) {
        Ok(child) => child,
        Err(e) => {
            window.set_recorder_status_text(
                format!("Failed to start {binary_name}: {e}").into(),
            );
            return;
        }
    };

    window.set_recorder_log_text("".into());
    window.set_recorder_status_text("".into());
    window.set_recorder_status_pill("Starting…".into());
    window.set_recorder_running(true);
    append_log(window, &format!("Started {binary_name}"));

    let (tx, rx) = mpsc::channel();
    process::stream_output(child, tx);

    let window_weak = window.as_weak();
    std::thread::spawn(move || {
        for msg in rx {
            let window_weak = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    handle_child_output(&window, msg);
                }
            });
        }
    });
}

/// Route one line of subprocess output: substring-match it for the
/// status pill (see docs/plans/acr-launcher-v1.md's "Recording panel"
/// section — "Waiting for ACC shared memory", "Connected to ACC shared
/// memory", "Recording to:", "Done. Recorded"), and always append it to
/// the raw log. `Exited` clears `recorder-running` so Start/Stop flip
/// back.
fn handle_child_output(window: &AppWindow, msg: ChildOutput) {
    match msg {
        ChildOutput::Stdout(line) | ChildOutput::Stderr(line) => {
            update_status_pill(window, &line);
            append_log(window, &line);
        }
        ChildOutput::Exited(code) => {
            window.set_recorder_running(false);
            let line = match code {
                Some(code) => format!("Process exited (code {code})"),
                None => "Process exited".to_string(),
            };
            append_log(window, &line);
            if window.get_recorder_status_pill() != "Stopped" {
                window.set_recorder_status_pill("Stopped".into());
            }
        }
    }
}

fn update_status_pill(window: &AppWindow, line: &str) {
    if line.contains("Waiting for ACC shared memory") {
        window.set_recorder_status_pill("Waiting for ACC/AC Rally…".into());
    } else if line.contains("Connected to ACC shared memory") {
        window.set_recorder_status_pill("Connected".into());
    } else if line.contains("Recording to:") {
        window.set_recorder_status_pill("Recording".into());
    } else if line.contains("Done. Recorded") {
        window.set_recorder_status_pill("Stopped".into());
    }
}

fn append_log(window: &AppWindow, line: &str) {
    let mut text = window.get_recorder_log_text().to_string();
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
    window.set_recorder_log_text(text.into());
}
