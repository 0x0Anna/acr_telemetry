//! Analysis Export tab: wraps `acr_analysis_export.exe`, a tool that
//! slices physics/graphics for a recording by Grafana annotation tag
//! (`rid_<recording_id>`) out of `telemetry.db` and writes them into
//! `analysis.db` (backing up the previous `analysis.db` first). CLI:
//! `acr_analysis_export <recording_id> [--grafana-db PATH]
//! [--telemetry-db PATH] [--analysis-db PATH]` for a one-shot run, or
//! `acr_analysis_export --serve [--port PORT] [...same path flags]` for a
//! long-running HTTP mode (see `src/bin/acr_analysis_export.rs`'s header
//! doc comment).
//!
//! **`--serve` is the mode the existing Grafana dashboards actually
//! expect**: `grafana/AC Rally full-dashboard.json` ships a dashboard
//! link ("Export Annotation ranges to analysis") wired to
//! `http://localhost:9876/export?recording_id=${recording_id}` — clicking
//! it in Grafana is the intended day-to-day trigger, not re-typing a
//! recording ID into this tab each time. Start/Stop here mirrors
//! `telemetry_bridge_panel.rs`'s shape exactly: spawn hidden, stream
//! output into a log + status pill, Stop writes a stop file (backend fix
//! in `src/bin/acr_analysis_export.rs` mirroring the phase-2
//! `acr_track_match`/phase-3 `acr_telemetry_bridge` stop-file convention)
//! rather than killing the process — spawned with no shared console, so
//! its own Ctrl+C handling can't be reached from here.
//!
//! The one-shot `<recording_id>` mode is kept alongside it for a manual,
//! ad-hoc export without needing `--serve` running first. Both modes'
//! output is human-readable status text on stderr only (a single line —
//! `"OK: N rows in analysis for recording R → path"` or an error) —
//! captured into a log/results panel, same pattern as
//! `grip_estimator_panel.rs`. `--telemetry-db`/`--grafana-db`/
//! `--analysis-db` are optional path overrides: the tool already resolves
//! sensible defaults (telemetry.db from `acr_recorder.toml`'s
//! `[export] sqlite_db_path`, analysis.db next to it, grafana.db from the
//! `GRAFANA_DB` env var) via `parse_paths` in the tool itself, so this
//! panel only passes a flag when the user has actually typed an override.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::{ComponentHandle, Weak};

use crate::process;
use crate::{AppState, AppWindow};

pub(crate) fn init(window: &AppWindow, state: Rc<RefCell<AppState>>) {
    let cfg = crate::launcher_config::load().analysis_export;
    if let Some(id) = &cfg.last_recording_id {
        window.set_analysis_export_recording_id(id.clone().into());
    }
    // `--grafana-db` is the one path the tool has no fallback for besides
    // the `GRAFANA_DB` env var (`--telemetry-db`/`--analysis-db` both fall
    // back to config-derived defaults — see `src/bin/acr_analysis_export.rs`'s
    // `parse_paths`) — pre-fill from the env var if it's set, same source
    // the tool itself checks, so the field only looks blank when a value
    // genuinely still needs typing in.
    if let Some(p) = &cfg.last_grafana_db {
        window.set_analysis_export_grafana_db(p.clone().into());
    } else if let Ok(env_path) = std::env::var("GRAFANA_DB") {
        window.set_analysis_export_grafana_db(env_path.into());
    }
    if let Some(p) = &cfg.last_telemetry_db {
        window.set_analysis_export_telemetry_db(p.clone().into());
    }
    if let Some(p) = &cfg.last_analysis_db {
        window.set_analysis_export_analysis_db(p.clone().into());
    }
    window.set_analysis_export_serve_port(cfg.last_serve_port.to_string().into());

    {
        let window_weak = window.as_weak();
        window.on_analysis_export_pick_grafana_db(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let dialog = rfd::FileDialog::new().add_filter("sqlite database", &["db", "sqlite", "sqlite3"]);
            if let Some(path) = dialog.pick_file() {
                window.set_analysis_export_grafana_db(path.to_string_lossy().into_owned().into());
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_analysis_export_pick_telemetry_db(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let dialog = rfd::FileDialog::new().add_filter("sqlite database", &["db", "sqlite", "sqlite3"]);
            if let Some(path) = dialog.pick_file() {
                window.set_analysis_export_telemetry_db(path.to_string_lossy().into_owned().into());
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_analysis_export_pick_analysis_db(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let dialog = rfd::FileDialog::new().add_filter("sqlite database", &["db", "sqlite", "sqlite3"]);
            if let Some(path) = dialog.save_file() {
                window.set_analysis_export_analysis_db(path.to_string_lossy().into_owned().into());
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_analysis_export_run(move || {
            let Some(window) = window_weak.upgrade() else { return };
            run(&window);
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_analysis_export_serve_start(move || {
            let Some(window) = window_weak.upgrade() else { return };

            if window.get_analysis_export_serve_running() {
                // Already have a child streaming; ignore a double-click.
                return;
            }

            serve_start(&window);
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_analysis_export_serve_stop(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let stop_path = serve_stop_file_path();
            match std::fs::write(&stop_path, b"") {
                Ok(()) => {
                    window.set_analysis_export_serve_status_pill("Stopping…".into());
                    append_serve_log(&window, &format!("Wrote stop file: {}", stop_path.display()));
                }
                Err(e) => {
                    window.set_analysis_export_serve_status_pill(
                        format!("Failed to write stop file: {e}").into(),
                    );
                }
            }
        });
    }

    let _ = state; // no AppState fields needed by this panel today
}

/// Same path `src/bin/acr_analysis_export.rs`'s own `stop_file_path()`
/// resolves to — duplicated here (rather than imported) because it's a
/// standalone `src/bin/` binary, not a `lib.rs` module, so there's
/// nothing for the launcher to `use`. Keep both copies in sync if the
/// convention changes.
fn serve_stop_file_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("acr_telemetry").join("acr_analysis_export_stop"))
        .unwrap_or_else(|| PathBuf::from(".acr_analysis_export_stop"))
}

/// Build `--serve`'s argument list from the same recording-scoped path
/// fields the one-shot Run button uses (`--grafana-db`/`--telemetry-db`/
/// `--analysis-db` are all still optional overrides here — `--serve` also
/// takes `recording_id` per-request via the query string, not as a CLI
/// arg), plus `--port`. Returns `None` (after setting a status error) if
/// the port isn't a valid number or Grafana DB is required but missing —
/// same validation the one-shot Run path uses.
fn serve_args(window: &AppWindow) -> Option<(Vec<String>, u16)> {
    let port_text = window.get_analysis_export_serve_port().to_string();
    let port: u16 = match port_text.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            window.set_analysis_export_serve_status_pill(
                format!("Error: port \"{port_text}\" must be a number 1-65535.").into(),
            );
            return None;
        }
    };

    let grafana_db = window.get_analysis_export_grafana_db().to_string();
    if grafana_db.trim().is_empty() && std::env::var("GRAFANA_DB").is_err() {
        window.set_analysis_export_serve_status_pill(
            "Error: Grafana DB path is required (no GRAFANA_DB env var is set).".into(),
        );
        return None;
    }

    let mut args: Vec<String> = vec!["--serve".to_string(), "--port".to_string(), port.to_string()];
    if !grafana_db.trim().is_empty() {
        args.push("--grafana-db".to_string());
        args.push(grafana_db);
    }
    let telemetry_db = window.get_analysis_export_telemetry_db().to_string();
    if !telemetry_db.trim().is_empty() {
        args.push("--telemetry-db".to_string());
        args.push(telemetry_db);
    }
    let analysis_db = window.get_analysis_export_analysis_db().to_string();
    if !analysis_db.trim().is_empty() {
        args.push("--analysis-db".to_string());
        args.push(analysis_db);
    }

    Some((args, port))
}

/// Spawn `acr_analysis_export.exe --serve ...` and start tailing its
/// output into the serve log/status pill, mirroring
/// `telemetry_bridge_panel.rs`'s `start`.
fn serve_start(window: &AppWindow) {
    let Some((args, port)) = serve_args(window) else { return };

    save_settings(window, Some(port));

    let binary = process::resolve_binary("acr_analysis_export.exe");
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let child = match process::spawn_hidden(&binary, &arg_refs) {
        Ok(child) => child,
        Err(e) => {
            window.set_analysis_export_serve_status_pill(
                format!("Failed to start acr_analysis_export.exe: {e}").into(),
            );
            return;
        }
    };

    window.set_analysis_export_serve_log("".into());
    window.set_analysis_export_serve_status_pill("Starting…".into());
    window.set_analysis_export_serve_running(true);
    append_serve_log(window, "Started acr_analysis_export.exe --serve");

    let window_weak = window.as_weak();
    std::thread::spawn(move || {
        let result = process::wait_for_output(child, |_is_stderr, line| {
            let window_weak = window_weak.clone();
            let line = line.to_string();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    update_serve_status_pill(&window, &line);
                    append_serve_log(&window, &line);
                }
            });
        });

        let window_weak = window_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window_weak.upgrade() {
                window.set_analysis_export_serve_running(false);
                let line = match result.exit_code {
                    Some(code) => format!("Process exited (code {code})"),
                    None => "Process exited".to_string(),
                };
                append_serve_log(&window, &line);
                window.set_analysis_export_serve_status_pill("Stopped".into());
            }
        });
    });
}

fn update_serve_status_pill(window: &AppWindow, line: &str) {
    if line.contains("acr_analysis_export on http://") {
        window.set_analysis_export_serve_status_pill("Running".into());
    } else if line.contains("Stop file detected") {
        window.set_analysis_export_serve_status_pill("Stopping…".into());
    }
}

fn append_serve_log(window: &AppWindow, line: &str) {
    crate::append_line!(window, get_analysis_export_serve_log, set_analysis_export_serve_log, line);
}

fn run(window: &AppWindow) {
    let recording_id = window.get_analysis_export_recording_id().to_string();
    if recording_id.trim().parse::<i64>().is_err() {
        window.set_analysis_export_status(
            format!("Error: recording ID \"{recording_id}\" must be a whole number.").into(),
        );
        return;
    }

    let grafana_db = window.get_analysis_export_grafana_db().to_string();
    if grafana_db.trim().is_empty() && std::env::var("GRAFANA_DB").is_err() {
        window.set_analysis_export_status(
            "Error: Grafana DB path is required (no GRAFANA_DB env var is set).".into(),
        );
        return;
    }

    let mut args: Vec<String> = vec![recording_id.trim().to_string()];

    if !grafana_db.trim().is_empty() {
        args.push("--grafana-db".to_string());
        args.push(grafana_db);
    }
    let telemetry_db = window.get_analysis_export_telemetry_db().to_string();
    if !telemetry_db.trim().is_empty() {
        args.push("--telemetry-db".to_string());
        args.push(telemetry_db);
    }
    let analysis_db = window.get_analysis_export_analysis_db().to_string();
    if !analysis_db.trim().is_empty() {
        args.push("--analysis-db".to_string());
        args.push(analysis_db);
    }

    save_settings(window, None);

    let binary = process::resolve_binary("acr_analysis_export.exe");

    window.set_analysis_export_running(true);
    window.set_analysis_export_status("Running…".into());
    window.set_analysis_export_results("".into());

    let window_weak = window.as_weak();
    std::thread::spawn(move || run_child(window_weak, binary, args));
}

/// Persist the recording ID + last-used path overrides (and, from
/// `serve_start`, the last-used port) so the tab pre-fills next launch —
/// this tool has no config file of its own, mirrors
/// `grip_estimator_panel.rs::save_settings`.
fn save_settings(window: &AppWindow, serve_port: Option<u16>) {
    let mut cfg = crate::launcher_config::load();
    let a = &mut cfg.analysis_export;
    let recording_id = window.get_analysis_export_recording_id().to_string();
    if !recording_id.trim().is_empty() {
        a.last_recording_id = Some(recording_id);
    }
    let grafana_db = window.get_analysis_export_grafana_db().to_string();
    if !grafana_db.trim().is_empty() {
        a.last_grafana_db = Some(grafana_db);
    }
    let telemetry_db = window.get_analysis_export_telemetry_db().to_string();
    if !telemetry_db.trim().is_empty() {
        a.last_telemetry_db = Some(telemetry_db);
    }
    let analysis_db = window.get_analysis_export_analysis_db().to_string();
    if !analysis_db.trim().is_empty() {
        a.last_analysis_db = Some(analysis_db);
    }
    if let Some(port) = serve_port {
        a.last_serve_port = port;
    }
    crate::launcher_config::save(&cfg);
}

fn run_child(window_weak: Weak<AppWindow>, binary: PathBuf, args: Vec<String>) {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    // The tool prints its one-line result (success or error) to stderr
    // only — see `src/bin/acr_analysis_export.rs::main`'s `eprintln!`/
    // `Err` paths — so, like `grip_estimator_panel.rs`, stdout is ignored
    // here (there isn't any) and stderr drives both the results panel and
    // `RunResult::failure_message`'s "Last output".
    let mut has_output = false;
    let result = process::run_and_wait(&binary, &arg_refs, |is_stderr, line| {
        if !is_stderr {
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
            // `run_export` prints exactly one `"OK: ..."` line either way —
            // including the no-op case where no Grafana annotation tagged
            // `rid_<id>` was found, which does *not* create/update
            // `analysis.db` (see `run_export`'s `ranges.is_empty()` branch
            // in `src/bin/acr_analysis_export.rs`). Surface it as the
            // status line, not just in the results panel below, so that
            // no-op reads as "nothing exported, here's why" rather than a
            // bare "Done." indistinguishable from a real export.
            let status = if has_output { r.last_line.clone() } else { "Done (no output).".to_string() };
            finish(&window_weak, true, status);
        }
        Ok(r) => finish(&window_weak, false, r.failure_message("acr_analysis_export")),
        Err(e) => finish(&window_weak, false, format!("Failed to launch {}: {e}", binary.display())),
    }
}

fn finish(window_weak: &Weak<AppWindow>, success: bool, status: String) {
    let window_weak = window_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window_weak.upgrade() {
            window.set_analysis_export_running(false);
            window.set_analysis_export_status(
                if success { status.into() } else { format!("Error: {status}").into() },
            );
        }
    });
}

fn append_results(window: &AppWindow, line: &str) {
    crate::append_line!(window, get_analysis_export_results, set_analysis_export_results, line);
}
