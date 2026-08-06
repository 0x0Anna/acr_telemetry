//! Grip Estimator tab (phase 3): wraps `acr_grip_estimator.exe`, a
//! one-shot tool that scores tire grip/traction from an existing
//! recording. Two mutually exclusive input modes (see
//! `src/bin/acr_grip_estimator.rs`'s hand-rolled `--flag value` parsing):
//! `--sqlite <path> [--recording-id <i64>]` or `--rkyv <path> [--track]
//! [--car]`, both taking shared `--early-sec`/`--correction-sec` flags.
//!
//! Output is CSV-formatted text on stdout only (no file) — a single batch
//! print at the end, not streamed progress — so this panel just captures
//! stdout into a monospace results panel rather than a structured table,
//! same log-panel pattern as `export_panel.rs`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::{ComponentHandle, Weak};

use crate::process;
use crate::{AppState, AppWindow};

pub(crate) fn init(window: &AppWindow, state: Rc<RefCell<AppState>>) {
    let cfg = crate::launcher_config::load().grip_estimator;
    window.set_grip_estimator_sqlite_mode(cfg.use_sqlite_mode);
    if let Some(p) = &cfg.last_sqlite_path {
        window.set_grip_estimator_sqlite_path(p.clone().into());
    }
    if let Some(p) = &cfg.last_rkyv_path {
        window.set_grip_estimator_rkyv_path(p.clone().into());
    }

    {
        let window_weak = window.as_weak();
        window.on_grip_estimator_pick_sqlite(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let dialog = rfd::FileDialog::new().add_filter("sqlite database", &["db", "sqlite", "sqlite3"]);
            if let Some(path) = dialog.pick_file() {
                window.set_grip_estimator_sqlite_path(path.to_string_lossy().into_owned().into());
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_grip_estimator_pick_rkyv(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let dialog = rfd::FileDialog::new().add_filter("rkyv recording", &["rkyv"]);
            if let Some(path) = dialog.pick_file() {
                window.set_grip_estimator_rkyv_path(path.to_string_lossy().into_owned().into());
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_grip_estimator_run(move || {
            let Some(window) = window_weak.upgrade() else { return };
            run(&window);
        });
    }

    let _ = state; // no AppState fields needed by this panel today
}

fn run(window: &AppWindow) {
    let sqlite_mode = window.get_grip_estimator_sqlite_mode();
    let early_sec = window.get_grip_estimator_early_sec().to_string();
    let correction_sec = window.get_grip_estimator_correction_sec().to_string();

    let mut args: Vec<String> = Vec::new();
    if sqlite_mode {
        let path = window.get_grip_estimator_sqlite_path().to_string();
        if path.trim().is_empty() {
            window.set_grip_estimator_status("Error: pick a sqlite database first.".into());
            return;
        }
        args.push("--sqlite".to_string());
        args.push(path);

        let recording_id = window.get_grip_estimator_recording_id().to_string();
        if !recording_id.trim().is_empty() {
            args.push("--recording-id".to_string());
            args.push(recording_id);
        }
    } else {
        let path = window.get_grip_estimator_rkyv_path().to_string();
        if path.trim().is_empty() {
            window.set_grip_estimator_status("Error: pick a .rkyv recording first.".into());
            return;
        }
        args.push("--rkyv".to_string());
        args.push(path);

        let track = window.get_grip_estimator_track().to_string();
        if !track.trim().is_empty() {
            args.push("--track".to_string());
            args.push(track);
        }
        let car = window.get_grip_estimator_car().to_string();
        if !car.trim().is_empty() {
            args.push("--car".to_string());
            args.push(car);
        }
    }

    if !early_sec.trim().is_empty() {
        args.push("--early-sec".to_string());
        args.push(early_sec);
    }
    if !correction_sec.trim().is_empty() {
        args.push("--correction-sec".to_string());
        args.push(correction_sec);
    }

    save_settings(window);

    let binary = process::resolve_binary("acr_grip_estimator.exe");

    window.set_grip_estimator_running(true);
    window.set_grip_estimator_status("Running…".into());
    window.set_grip_estimator_results("".into());

    let window_weak = window.as_weak();
    std::thread::spawn(move || run_child(window_weak, binary, args));
}

/// Persist the mode + last-used paths so the tab pre-fills next launch —
/// this tool has no config file of its own, so it's the only persistence
/// available (mirrors `track_match_panel.rs`'s "Save references" idea, but
/// automatic on every Run rather than a separate button, since there's no
/// other natural place to save it from).
fn save_settings(window: &AppWindow) {
    let mut cfg = crate::launcher_config::load();
    cfg.grip_estimator.use_sqlite_mode = window.get_grip_estimator_sqlite_mode();
    let sqlite_path = window.get_grip_estimator_sqlite_path().to_string();
    if !sqlite_path.trim().is_empty() {
        cfg.grip_estimator.last_sqlite_path = Some(sqlite_path);
    }
    let rkyv_path = window.get_grip_estimator_rkyv_path().to_string();
    if !rkyv_path.trim().is_empty() {
        cfg.grip_estimator.last_rkyv_path = Some(rkyv_path);
    }
    crate::launcher_config::save(&cfg);
}

fn run_child(window_weak: Weak<AppWindow>, binary: PathBuf, args: Vec<String>) {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    // Only stdout carries the tool's actual (batch-printed) CSV output —
    // stderr is still tracked for `RunResult::failure_message`'s "Last
    // output" but never shown in the results panel.
    let mut has_output = false;
    let result = process::run_and_wait(&binary, &arg_refs, |is_stderr, line| {
        if is_stderr {
            return;
        }
        has_output = true;
        let window_weak = window_weak.clone();
        let line = line.to_string();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window_weak.upgrade() {
                append_results(&window, &line);
            }
        });
    });

    match result {
        Ok(r) if r.succeeded() => {
            let status = if has_output { "Done." } else { "Done (no output)." };
            finish(&window_weak, true, status.to_string());
        }
        Ok(r) => finish(&window_weak, false, r.failure_message("acr_grip_estimator")),
        Err(e) => finish(&window_weak, false, format!("Failed to launch {}: {e}", binary.display())),
    }
}

fn finish(window_weak: &Weak<AppWindow>, success: bool, status: String) {
    let window_weak = window_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window_weak.upgrade() {
            window.set_grip_estimator_running(false);
            window.set_grip_estimator_status(
                if success { status.into() } else { format!("Error: {status}").into() },
            );
        }
    });
}

fn append_results(window: &AppWindow, line: &str) {
    crate::append_line!(window, get_grip_estimator_results, set_grip_estimator_results, line);
}
