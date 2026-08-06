//! Telemetry Bridge tab (phase 3): wraps `acr_telemetry_bridge.exe`, a
//! long-running server that reads ACC/AC Rally shared memory and serves
//! it over UDP and/or HTTP for a phone/second-screen dashboard (see
//! `docs/BRIDGE.md`, `src/bin/acr_telemetry_bridge.rs`). Needs the sim
//! already running and connectable when started — it does not retry, per
//! `ACCSharedMemory::new()` being called once at startup and returning
//! `Err` (process exit) if the shared memory isn't up yet.
//!
//! Start/Stop lifecycle mirrors `recorder_panel.rs`: write the UI's
//! settings into `acr_telemetry_bridge.toml` (so the spawned process picks
//! them up with no CLI flags, avoiding flag/TOML duplication), spawn
//! hidden, stream output into a log + status pill. Stop writes a stop
//! file (backend fix in `src/bin/acr_telemetry_bridge.rs` mirroring the
//! phase-2 `acr_track_match` fix) rather than killing the process — the
//! bridge is spawned with no shared console, so its own Ctrl+C handler
//! can't be reached from here.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::process;
use crate::{AppState, AppWindow};

pub(crate) fn init(window: &AppWindow, state: Rc<RefCell<AppState>>) {
    let cfg = crate::launcher_config::load().telemetry_bridge;
    window.set_telemetry_bridge_rate_hz(cfg.rate_hz.to_string().into());
    window.set_telemetry_bridge_udp_enabled(cfg.udp_enabled);
    window.set_telemetry_bridge_udp_target(cfg.udp_target.into());
    window.set_telemetry_bridge_http_enabled(cfg.http_enabled);
    window.set_telemetry_bridge_http_addr(cfg.http_addr.into());
    window.set_telemetry_bridge_temp_unit(cfg.temperature_unit.into());

    {
        let window_weak = window.as_weak();
        window.on_telemetry_bridge_start(move || {
            let Some(window) = window_weak.upgrade() else { return };

            if window.get_telemetry_bridge_running() {
                // Already have a child streaming; ignore a double-click.
                return;
            }

            start(&window);
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_telemetry_bridge_stop(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let stop_path = stop_file_path();
            match std::fs::write(&stop_path, b"") {
                Ok(()) => {
                    window.set_telemetry_bridge_status_pill("Stopping…".into());
                    append_log(&window, &format!("Wrote stop file: {}", stop_path.display()));
                }
                Err(e) => {
                    window.set_telemetry_bridge_status_text(
                        format!("Failed to write stop file: {e}").into(),
                    );
                }
            }
        });
    }

    let _ = state; // no AppState fields needed by this panel today
}

/// Same path `src/bin/acr_telemetry_bridge.rs`'s own `stop_file_path()`
/// resolves to — duplicated here (rather than imported) because the
/// bridge is a standalone `src/bin/` binary, not a `lib.rs` module like
/// `acr_recorder::track_match_app`, so there's nothing for the launcher to
/// `use`. Keep both copies in sync if the convention changes.
fn stop_file_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("acr_telemetry").join("acr_telemetry_bridge_stop"))
        .unwrap_or_else(|| PathBuf::from(".acr_telemetry_bridge_stop"))
}

/// Where `acr_telemetry_bridge.toml` is written back to: next to the
/// launcher's own executable, same convention `recorder_panel.rs` uses
/// for `acr_recorder.toml`, and the first path
/// `config::load_bridge_config()` searches.
fn config_file_path() -> PathBuf {
    acr_recorder::config::base_dir()
        .map(|dir| dir.join("acr_telemetry_bridge.toml"))
        .unwrap_or_else(|| PathBuf::from("acr_telemetry_bridge.toml"))
}

/// Mirrors `acr_recorder::config::BridgeConfig`'s shape (`src/config.rs`)
/// closely enough to round-trip through `toml::to_string_pretty` into the
/// exact file `acr_telemetry_bridge`'s `config::load_bridge_config()`
/// reads. `dashboard_slots`/`telemetry_colors` are deliberately omitted
/// (left at the tool's own defaults via `#[serde(default)]` on read) —
/// advanced, TOML-only, out of scope for the v1 tab UI per the phase-3
/// plan (same "config-first, don't expose everything" precedent as Track
/// Match's v1 scope).
#[derive(serde::Serialize)]
struct BridgeTomlOut {
    rate_hz: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    udp_target: Option<String>,
    http_addr: String,
    temperature_unit: String,
}

fn write_bridge_config(window: &AppWindow) -> std::io::Result<PathBuf> {
    let rate_hz: u64 = window.get_telemetry_bridge_rate_hz().parse().unwrap_or(5).max(1);
    let udp_target = if window.get_telemetry_bridge_udp_enabled() {
        let t = window.get_telemetry_bridge_udp_target().to_string();
        if t.trim().is_empty() { None } else { Some(t) }
    } else {
        None
    };
    let http_addr = if window.get_telemetry_bridge_http_enabled() {
        let a = window.get_telemetry_bridge_http_addr().to_string();
        if a.trim().is_empty() { "0.0.0.0:8080".to_string() } else { a }
    } else {
        String::new()
    };
    let temperature_unit = window.get_telemetry_bridge_temp_unit().to_string();

    let out = BridgeTomlOut { rate_hz, udp_target, http_addr, temperature_unit };
    let path = config_file_path();
    let text = toml::to_string_pretty(&out)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, &text)?;
    Ok(path)
}

/// Persist the UI's settings into `acr_launcher.toml` so they pre-fill
/// next launch (independent of `acr_telemetry_bridge.toml`, which gets
/// overwritten fresh from these same values on every Start).
fn save_ui_settings(window: &AppWindow) {
    let mut cfg = crate::launcher_config::load();
    let b = &mut cfg.telemetry_bridge;
    b.rate_hz = window.get_telemetry_bridge_rate_hz().parse().unwrap_or(b.rate_hz).max(1);
    b.udp_enabled = window.get_telemetry_bridge_udp_enabled();
    b.udp_target = window.get_telemetry_bridge_udp_target().to_string();
    b.http_enabled = window.get_telemetry_bridge_http_enabled();
    b.http_addr = window.get_telemetry_bridge_http_addr().to_string();
    b.temperature_unit = window.get_telemetry_bridge_temp_unit().to_string();
    crate::launcher_config::save(&cfg);
}

fn start(window: &AppWindow) {
    save_ui_settings(window);

    if let Err(e) = write_bridge_config(window) {
        window.set_telemetry_bridge_status_text(
            format!("Failed to write acr_telemetry_bridge.toml: {e}").into(),
        );
        return;
    }

    let binary_path = process::resolve_binary("acr_telemetry_bridge.exe");

    let child = match process::spawn_hidden(&binary_path, &[]) {
        Ok(child) => child,
        Err(e) => {
            window.set_telemetry_bridge_status_text(
                format!("Failed to start acr_telemetry_bridge.exe: {e}").into(),
            );
            return;
        }
    };

    window.set_telemetry_bridge_log("".into());
    window.set_telemetry_bridge_status_text("".into());
    window.set_telemetry_bridge_status_pill("Starting…".into());
    window.set_telemetry_bridge_running(true);
    append_log(window, "Started acr_telemetry_bridge.exe");

    let window_weak = window.as_weak();
    std::thread::spawn(move || {
        // Substring-match the exact `eprintln!` lines
        // `src/bin/acr_telemetry_bridge.rs` prints ("Bridge running at"
        // once the read loop starts, "failed to bind" on a non-fatal HTTP
        // bind failure — the UDP/state loop keeps going even then, so this
        // doesn't flip the pill to stopped, only surfaces the error in the
        // log — and this panel's own "Stop file detected" from the backend
        // fix) for the status pill, and always append the line to the raw log.
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
                window.set_telemetry_bridge_running(false);
                let line = match result.exit_code {
                    Some(code) => format!("Process exited (code {code})"),
                    None => "Process exited".to_string(),
                };
                append_log(&window, &line);
                window.set_telemetry_bridge_status_pill("Stopped".into());
            }
        });
    });
}

fn update_status_pill(window: &AppWindow, line: &str) {
    if line.contains("Bridge running at") {
        window.set_telemetry_bridge_status_pill("Running".into());
    } else if line.contains("failed to bind") {
        window.set_telemetry_bridge_status_text(line.to_string().into());
    } else if line.contains("Stop file detected") {
        window.set_telemetry_bridge_status_pill("Stopping…".into());
    }
}

fn append_log(window: &AppWindow, line: &str) {
    crate::append_line!(window, get_telemetry_bridge_log, set_telemetry_bridge_log, line);
}
