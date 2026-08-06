//! Track Match tab (section 3 of `docs/plans/acr-launcher-phase2.md`):
//! wraps `acr_track_match.exe`, a long-running live-matching/timing tool
//! in `--live` mode and a one-shot batch matcher in `--input FILE.rkyv`
//! mode. Only `--refs` and the live/offline choice are exposed — the
//! other ~35 CLI flags stay TOML-driven (`acr_track_match.toml` /
//! `acr_timing.toml` / `acr_pacenotes.toml`), matching how the Record tab
//! intentionally left distance-reset tuning out of its UI.
//!
//! **Live mode** mirrors `recorder_panel.rs`'s exact shape: Start spawns
//! the child and streams its output into a log + status pill (substring
//! matching on real `eprintln!` lines from `track_match_app.rs`), Stop
//! writes a stop file rather than killing the process — the file
//! `acr_recorder::track_match_app::stop_file_path()` resolves to, which
//! `track_match_app.rs`'s `--live` loop now polls once a second (see the
//! backend fix in the same commit series).
//!
//! **Offline mode** mirrors `export_panel.rs`'s fire-and-forget shape:
//! Run spawns the child with `--input <file>`, streams output to the
//! same log, and the process exits on its own once matching completes —
//! no Stop button, no persistent "running" state beyond the one process.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

use slint::{ComponentHandle, Weak};

use crate::process::{self, ChildOutput};
use crate::{AppState, AppWindow};

/// Register all `track-match-*` callbacks on `window`. Call once from
/// `main()` after the window is constructed, alongside the other panels'
/// `init` calls.
pub(crate) fn init(window: &AppWindow, state: Rc<RefCell<AppState>>) {
    // Pre-fill from the last references saved via "Save references" (see
    // `on_track_match_save_refs` below) — stored in the launcher-only
    // acr_launcher.toml since the launcher always passes `--refs`
    // explicitly, so this is purely "remember what I picked last time".
    let remembered_refs = crate::launcher_config::load().track_match.refs;
    if !remembered_refs.is_empty() {
        let paths: Vec<PathBuf> = remembered_refs.into_iter().map(PathBuf::from).collect();
        set_refs_label(window, &paths);
        state.borrow_mut().track_match_refs = paths;
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_track_match_save_refs(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let refs = state.borrow().track_match_refs.clone();
            if refs.is_empty() {
                window.set_track_match_settings_status(
                    "Error: pick reference track file(s) or a folder first.".into(),
                );
                return;
            }
            let mut cfg = crate::launcher_config::load();
            cfg.track_match.refs =
                refs.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            crate::launcher_config::save(&cfg);
            window.set_track_match_settings_status("Saved.".into());
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_track_match_pick_refs_files(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let files = rfd::FileDialog::new()
                .add_filter("reference track", &["rkyv", "shp"])
                .pick_files();
            if let Some(files) = files {
                if !files.is_empty() {
                    set_refs_label(&window, &files);
                    state.borrow_mut().track_match_refs = files;
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_track_match_pick_refs_folder(move || {
            let Some(window) = window_weak.upgrade() else { return };
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                set_refs_label(&window, std::slice::from_ref(&dir));
                state.borrow_mut().track_match_refs = vec![dir];
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_track_match_pick_input(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let dialog = rfd::FileDialog::new().add_filter("rkyv recording", &["rkyv"]);
            if let Some(path) = dialog.pick_file() {
                window.set_track_match_input_path(path.to_string_lossy().into_owned().into());
                state.borrow_mut().track_match_input = Some(path);
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_track_match_start(move || {
            let Some(window) = window_weak.upgrade() else { return };

            if window.get_track_match_running() {
                // Already have a child streaming; ignore a double-click.
                return;
            }

            let refs = state.borrow().track_match_refs.clone();
            let Some(refs_arg) = refs_arg(&refs) else {
                window.set_track_match_status_text(
                    "Error: pick reference track file(s) or a folder first.".into(),
                );
                return;
            };

            start_live(&window, refs_arg);
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_track_match_stop(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let stop_path = acr_recorder::track_match_app::stop_file_path();
            match std::fs::write(&stop_path, b"") {
                Ok(()) => {
                    window.set_track_match_status_pill("Stopping…".into());
                    append_log(&window, &format!("Wrote stop file: {}", stop_path.display()));
                }
                Err(e) => {
                    window.set_track_match_status_text(
                        format!("Failed to write stop file: {e}").into(),
                    );
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_track_match_run(move || {
            let Some(window) = window_weak.upgrade() else { return };

            if window.get_track_match_offline_running() {
                return;
            }

            let (refs, input) = {
                let s = state.borrow();
                (s.track_match_refs.clone(), s.track_match_input.clone())
            };
            let Some(refs_arg) = refs_arg(&refs) else {
                window.set_track_match_offline_status(
                    "Error: pick reference track file(s) or a folder first.".into(),
                );
                return;
            };
            let Some(input) = input else {
                window.set_track_match_offline_status(
                    "Error: pick a .rkyv input file first.".into(),
                );
                return;
            };

            run_offline(&window, refs_arg, input);
        });
    }
}

/// Comma-joined `--refs` argument value from the picked paths (files or a
/// single directory), matching `acr_track_match --refs` `path[,path...]`
/// contract (see `src/track_match_app.rs`'s `parse_args`/
/// `resolve_reference_files`). `None` if nothing has been picked yet.
fn refs_arg(refs: &[PathBuf]) -> Option<String> {
    if refs.is_empty() {
        return None;
    }
    Some(
        refs.iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(","),
    )
}

fn set_refs_label(window: &AppWindow, paths: &[PathBuf]) {
    let label = if paths.len() == 1 {
        format!("References: {}", paths[0].display())
    } else {
        format!(
            "References ({} files): {}",
            paths.len(),
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    window.set_track_match_refs_label(label.into());
}

/// Spawn `acr_track_match.exe --refs <refs_arg> --live` and start tailing
/// its output into the log panel / status pill, mirroring
/// `recorder_panel.rs`'s `start_recording`.
fn start_live(window: &AppWindow, refs_arg: String) {
    let binary_path = process::resolve_binary("acr_track_match.exe");
    let args = ["--refs", refs_arg.as_str(), "--live"];

    let child = match process::spawn_hidden(&binary_path, &args) {
        Ok(child) => child,
        Err(e) => {
            window.set_track_match_status_text(
                format!("Failed to start acr_track_match.exe: {e}").into(),
            );
            return;
        }
    };

    window.set_track_match_log_text("".into());
    window.set_track_match_status_text("".into());
    window.set_track_match_status_pill("Starting…".into());
    window.set_track_match_running(true);
    append_log(window, "Started acr_track_match.exe --live");

    let (tx, rx) = mpsc::channel();
    process::stream_output(child, tx);

    let window_weak = window.as_weak();
    std::thread::spawn(move || {
        for msg in rx {
            let window_weak = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    handle_live_output(&window, msg);
                }
            });
        }
    });
}

/// Route one line of the live child's output: substring-match it for the
/// status pill (see `src/track_match_app.rs`'s `eprintln!` lines this
/// mirrors — "live mode started" while waiting on ACC shared memory,
/// "ACC telemetry active" once physics packets are flowing, "start
/// armed:" when a timing session arms at a start anchor, "track locked:"
/// once geometry match settles on a reference track, and this panel's own
/// "Stop file detected" from the backend fix), and always append it to
/// the raw log. `Exited` clears `track-match-running` so Start/Stop flip
/// back.
fn handle_live_output(window: &AppWindow, msg: ChildOutput) {
    match msg {
        ChildOutput::Stdout(line) | ChildOutput::Stderr(line) => {
            update_status_pill(window, &line);
            append_log(window, &line);
        }
        ChildOutput::Exited(code) => {
            window.set_track_match_running(false);
            let line = match code {
                Some(code) => format!("Process exited (code {code})"),
                None => "Process exited".to_string(),
            };
            append_log(window, &line);
            window.set_track_match_status_pill("Stopped".into());
        }
    }
}

fn update_status_pill(window: &AppWindow, line: &str) {
    if line.contains("live mode started") {
        window.set_track_match_status_pill("Waiting for ACC/AC Rally…".into());
    } else if line.contains("ACC telemetry active") {
        window.set_track_match_status_pill("Connected".into());
    } else if line.contains("start armed:") {
        window.set_track_match_status_pill("Session armed".into());
    } else if line.contains("track locked:") {
        window.set_track_match_status_pill("Track locked".into());
    } else if line.contains("Stop file detected") {
        window.set_track_match_status_pill("Stopping…".into());
    }
}

/// Spawn `acr_track_match.exe --refs <refs_arg> --input <input>` and
/// stream its output into the log, mirroring `export_panel.rs`'s
/// fire-and-forget `run_export`/`run_queue` shape — the process exits on
/// its own once matching completes, no Stop button involved.
fn run_offline(window: &AppWindow, refs_arg: String, input: PathBuf) {
    let binary = process::resolve_binary("acr_track_match.exe");
    let input_str = input.to_string_lossy().into_owned();

    window.set_track_match_offline_running(true);
    window.set_track_match_offline_status("Running…".into());
    window.set_track_match_log_text("".into());

    let window_weak = window.as_weak();
    std::thread::spawn(move || run_offline_queue(window_weak, binary, refs_arg, input_str));
}

fn run_offline_queue(window_weak: Weak<AppWindow>, binary: PathBuf, refs_arg: String, input: String) {
    let args = ["--refs", refs_arg.as_str(), "--input", input.as_str()];

    let child = match process::spawn_hidden(&binary, &args) {
        Ok(child) => child,
        Err(e) => {
            let msg = format!("Failed to launch {}: {e}", binary.display());
            finish_offline(&window_weak, false, msg);
            return;
        }
    };

    let (tx, rx) = mpsc::channel();
    process::stream_output(child, tx);

    let mut exit_code: Option<i32> = None;
    let mut last_line = String::new();
    for msg in rx {
        match msg {
            ChildOutput::Stdout(line) | ChildOutput::Stderr(line) => {
                last_line = line.clone();
                let window_weak = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = window_weak.upgrade() {
                        append_log(&window, &line);
                    }
                });
            }
            ChildOutput::Exited(code) => {
                exit_code = code;
            }
        }
    }

    if exit_code == Some(0) {
        finish_offline(&window_weak, true, "Done.".to_string());
    } else {
        let msg = match exit_code {
            Some(code) => format!("acr_track_match exited with code {code}. Last output: {last_line}"),
            None => format!("acr_track_match exited abnormally. Last output: {last_line}"),
        };
        finish_offline(&window_weak, false, msg);
    }
}

fn finish_offline(window_weak: &Weak<AppWindow>, success: bool, status: String) {
    let window_weak = window_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window_weak.upgrade() {
            window.set_track_match_offline_running(false);
            window.set_track_match_offline_status(
                if success { status.into() } else { format!("Error: {status}").into() },
            );
        }
    });
}

fn append_log(window: &AppWindow, line: &str) {
    let mut text = window.get_track_match_log_text().to_string();
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
    window.set_track_match_log_text(text.into());
}
