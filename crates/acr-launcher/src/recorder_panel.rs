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

use slint::ComponentHandle;

use crate::process;
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
            let warning = apply_ui_to_config(&window, &mut app_state.config);
            match save_config(&app_state.config) {
                Ok(path) => {
                    let mut status = format!("Saved to {}", path.display());
                    if let Some(w) = warning {
                        status = format!("{status} ({w})");
                    }
                    window.set_recorder_status_text(status.into());
                }
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

            let warning = {
                let mut app_state = state.borrow_mut();
                let warning = apply_ui_to_config(&window, &mut app_state.config);
                if let Err(e) = save_config(&app_state.config) {
                    window.set_recorder_status_text(format!("Save failed: {e}").into());
                    return;
                }
                warning
            };

            start_recording(&window, warning);
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
    window.set_recorder_motec_profile(cfg.export.motec.profile.clone().into());
}

/// Read the Record tab's editable properties back into `cfg.recorder`.
/// Leaves fields the panel doesn't expose (distance-reset tuning, etc.)
/// untouched. Returns a warning to surface alongside the save/start
/// status if `ring_slots` didn't parse — the old value is kept rather
/// than silently discarding the user's edit without saying so.
fn apply_ui_to_config(window: &AppWindow, cfg: &mut acr_recorder::config::Config) -> Option<String> {
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

    let mut warning = None;
    let slots_text = window.get_recorder_ring_slots().to_string();
    match slots_text.parse::<usize>() {
        Ok(slots) => r.ring_slots = slots.max(2),
        Err(_) => {
            warning = Some(format!(
                "ring slots \"{slots_text}\" isn't a number; kept {}",
                r.ring_slots
            ));
        }
    }

    let prefix = window.get_recorder_ring_prefix().to_string();
    if !prefix.trim().is_empty() {
        r.ring_prefix = prefix;
    }

    let profile = window.get_recorder_motec_profile().to_string();
    if !profile.trim().is_empty() {
        cfg.export.motec.profile = profile;
    }

    warning
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

/// Write only the fields the Record tab actually edits
/// (`[recorder]`'s `raw_output_dir`/`notes_dir`/`record_graphics`/
/// `ring_mode`/`ring_slots`/`ring_prefix` and `[export.motec]`'s
/// `profile`) into the on-disk `acr_recorder.toml` via `toml_edit`,
/// rather than round-tripping the whole `Config` through
/// `toml::to_string_pretty`. Every other key/table/comment already in the
/// file — including sections this GUI doesn't expose, like distance-reset
/// tuning — passes through untouched, closing the comment/formatting-loss
/// trade-off the v1 plan flagged as a known limitation.
pub(crate) fn save_config(cfg: &acr_recorder::config::Config) -> std::io::Result<PathBuf> {
    let path = config_file_path();
    let mut doc = load_document(&path);

    let recorder = doc["recorder"]
        .or_insert(toml_edit::table())
        .as_table_mut()
        .expect("recorder section must be a table");
    recorder["raw_output_dir"] = toml_edit::value(cfg.recorder.raw_output_dir.clone());
    match &cfg.recorder.notes_dir {
        Some(dir) => recorder["notes_dir"] = toml_edit::value(dir.clone()),
        None => {
            recorder.remove("notes_dir");
        }
    }
    recorder["record_graphics"] = toml_edit::value(cfg.recorder.record_graphics);
    recorder["ring_mode"] = toml_edit::value(cfg.recorder.ring_mode);
    recorder["ring_slots"] = toml_edit::value(cfg.recorder.ring_slots as i64);
    recorder["ring_prefix"] = toml_edit::value(cfg.recorder.ring_prefix.clone());

    let export = doc["export"]
        .or_insert(toml_edit::table())
        .as_table_mut()
        .expect("export section must be a table");
    let motec = export["motec"]
        .or_insert(toml_edit::table())
        .as_table_mut()
        .expect("export.motec section must be a table");
    motec["profile"] = toml_edit::value(cfg.export.motec.profile.clone());

    std::fs::write(&path, doc.to_string())?;
    Ok(path)
}

/// Parse the existing `acr_recorder.toml` (if any) into an editable
/// `toml_edit` document so [`save_config`] can patch specific keys in
/// place. Falls back to a fresh, empty document if the file doesn't exist
/// yet or fails to parse (e.g. was hand-corrupted) — `save_config` then
/// writes only the keys it knows about, and `load_config` fills in
/// everything else from `Config`'s `#[serde(default)]`s on next read.
fn load_document(path: &std::path::Path) -> toml_edit::DocumentMut {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .unwrap_or_default()
}

/// Spawn the chosen binary (`acr_motec.exe` if "Record directly to
/// MoTeC" is checked, `acr_recorder.exe` otherwise) and start tailing its
/// output into the log panel / status pill. `config_warning` (e.g. an
/// unparseable ring-slots value) is logged once recording starts rather
/// than shown in `status_text`, which this function clears immediately
/// below.
fn start_recording(window: &AppWindow, config_warning: Option<String>) {
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
    if let Some(w) = config_warning {
        append_log(window, &format!("Warning: {w}"));
    }

    let window_weak = window.as_weak();
    std::thread::spawn(move || {
        let result = process::wait_for_output(child, |_is_stderr, line| {
            let window_weak = window_weak.clone();
            let line = line.to_string();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    update_status_pill(&window, &line);
                    append_log(&window, &line);
                }
            });
        });

        let window_weak = window_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window_weak.upgrade() {
                window.set_recorder_running(false);
                let line = match result.exit_code {
                    Some(code) => format!("Process exited (code {code})"),
                    None => "Process exited".to_string(),
                };
                append_log(&window, &line);
                if window.get_recorder_status_pill() != "Stopped" {
                    window.set_recorder_status_pill("Stopped".into());
                }
            }
        });
    });
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
    crate::append_line!(window, get_recorder_log_text, set_recorder_log_text, line);
}
