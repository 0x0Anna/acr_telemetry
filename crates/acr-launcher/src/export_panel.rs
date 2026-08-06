//! Export tab: picks an input (single `.rkyv` file, a directory, or the
//! configured `raw_output_dir` for a full batch), runs `acr_export.exe`
//! for each checked method (CSV, SQLite), and streams its output into
//! the log panel. See `src/bin/acr_export.rs`'s header doc comment for
//! the exact CLI contract this mirrors, and `docs/EXPORT.md`.
//!
//! `acr_export` only accepts one export format per run ("Use exactly one
//! export format: --csv, --sqlite, or --shp") — see
//! `src/bin/acr_export.rs`'s `parse_args`. Since the plan's UI calls for
//! independent CSV/SQLite checkboxes (not a single radio choice), this
//! panel runs one `acr_export` invocation per checked method,
//! sequentially, rather than trying to combine them into one call.
//!
//! Follows the same "background thread + `slint::invoke_from_event_loop`
//! per message" pattern as `main.rs`'s `spawn_status_poll`, since a Slint
//! window handle isn't `Send` and can't be touched directly from a
//! background thread.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

use slint::{ComponentHandle, Weak};

use crate::process::{self, ChildOutput};
use crate::{AppState, AppWindow};

/// What the user picked as the export input, mirroring `acr_export`'s
/// three invocation shapes (single file, directory batch, `--rawDir`
/// batch from config).
#[derive(Debug, Clone)]
pub enum ExportInput {
    File(PathBuf),
    Directory(PathBuf),
    RawDir,
}

impl ExportInput {
    /// The positional/`--rawDir` argument(s) `acr_export` expects for
    /// this input, before any `--csv`/`--sqlite` method flag is appended.
    fn base_args(&self) -> Vec<String> {
        match self {
            ExportInput::File(p) => vec![p.to_string_lossy().into_owned()],
            ExportInput::Directory(p) => vec![p.to_string_lossy().into_owned()],
            ExportInput::RawDir => vec!["--rawDir".to_string()],
        }
    }

    /// Directory the exported files land in, for the "Open output
    /// folder" button: the source file/dir itself for file/dir mode, or
    /// the configured raw dir (resolved the same way `acr_export` itself
    /// resolves it) for `--rawDir` mode.
    fn output_dir(&self, cfg: &acr_recorder::config::Config) -> PathBuf {
        match self {
            ExportInput::File(p) => p
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| p.clone()),
            ExportInput::Directory(p) => p.clone(),
            ExportInput::RawDir => acr_recorder::config::resolve_path(&cfg.recorder.raw_output_dir),
        }
    }
}

/// Register all `export-*` callbacks on `window`, wiring them against
/// the shared `state`. Also pre-fills `export-sqlite-path` from config
/// and the raw-dir hint used by the "use raw dir" button. Call once from
/// `main()` after the window is constructed.
pub(crate) fn init(window: &AppWindow, state: Rc<RefCell<AppState>>) {
    let sqlite_default = {
        let s = state.borrow();
        acr_recorder::config::resolve_path(&s.config.export.sqlite_db_path)
            .to_string_lossy()
            .into_owned()
    };
    window.set_export_sqlite_path(sqlite_default.into());

    let out_dir_default = {
        let s = state.borrow();
        let dir = &s.config.export.output_dir;
        if dir.trim().is_empty() {
            String::new()
        } else {
            acr_recorder::config::resolve_path(dir)
                .to_string_lossy()
                .into_owned()
        }
    };
    window.set_export_out_dir(out_dir_default.into());

    // Which export methods were checked last time — the sqlite/output-dir
    // *paths* above already come from the shared acr_recorder.toml, but
    // there's no natural home for two independent checkboxes there, so
    // this bit of UI memory lives in the launcher-only acr_launcher.toml
    // instead (see launcher_config.rs's ExportUiConfig doc comment).
    let export_ui_cfg = crate::launcher_config::load().export;
    window.set_export_do_csv(export_ui_cfg.do_csv);
    window.set_export_do_sqlite(export_ui_cfg.do_sqlite);

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_export_save(move || {
            let Some(window) = window_weak.upgrade() else { return };
            save_export_settings(&window, &state);
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_export_pick_out_dir(move || {
            let Some(window) = window_weak.upgrade() else { return };
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                window.set_export_out_dir(path.to_string_lossy().into_owned().into());
            }
        });
    }
    {
        let window_weak = window.as_weak();
        window.on_export_clear_out_dir(move || {
            let Some(window) = window_weak.upgrade() else { return };
            window.set_export_out_dir("".into());
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_export_pick_file(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let raw_dir = {
                let s = state.borrow();
                acr_recorder::config::resolve_path(&s.config.recorder.raw_output_dir)
            };
            let mut dialog = rfd::FileDialog::new().add_filter("rkyv recording", &["rkyv"]);
            if raw_dir.exists() {
                dialog = dialog.set_directory(&raw_dir);
            }
            if let Some(path) = dialog.pick_file() {
                window.set_export_mode_label(format!("File: {}", path.display()).into());
                state.borrow_mut().export_input = Some(ExportInput::File(path));
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_export_pick_folder(move || {
            let Some(window) = window_weak.upgrade() else { return };
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                window.set_export_mode_label(
                    format!("Directory (batch): {}", path.display()).into(),
                );
                state.borrow_mut().export_input = Some(ExportInput::Directory(path));
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_export_use_raw_dir(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let raw_dir = {
                let s = state.borrow();
                acr_recorder::config::resolve_path(&s.config.recorder.raw_output_dir)
            };
            window.set_export_mode_label(
                format!("Whole raw dir (batch, from config): {}", raw_dir.display()).into(),
            );
            state.borrow_mut().export_input = Some(ExportInput::RawDir);
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_export_run(move || {
            let Some(window) = window_weak.upgrade() else { return };
            run_export(&window, &state);
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_export_open_output_folder(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let dir = window.get_export_output_dir().to_string();
            if !dir.is_empty() {
                // Windows-only crate (matches the rest of the workspace's
                // #[cfg(windows)] conventions) — `explorer` is always
                // available.
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
        });
    }
}

/// Persist the Export tab's settings so they survive a restart: the
/// SQLite DB path and CSV/LD/SHP output-dir override go into the shared
/// `acr_recorder.toml` (`[export]`, alongside whatever the Record tab last
/// saved there — mirrors `recorder_panel.rs`'s `on_recorder_save`), while
/// the CSV/SQLite checkboxes go into the launcher-only `acr_launcher.toml`
/// since they have no natural home in the CLI tools' shared config.
fn save_export_settings(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    {
        let mut app_state = state.borrow_mut();
        app_state.config.export.sqlite_db_path = window.get_export_sqlite_path().to_string();
        app_state.config.export.output_dir = window.get_export_out_dir().to_string();
        if let Err(e) = crate::recorder_panel::save_config(&app_state.config) {
            window.set_export_settings_status(format!("Save failed: {e}").into());
            return;
        }
    }

    let mut launcher_cfg = crate::launcher_config::load();
    launcher_cfg.export.do_csv = window.get_export_do_csv();
    launcher_cfg.export.do_sqlite = window.get_export_do_sqlite();
    crate::launcher_config::save(&launcher_cfg);

    window.set_export_settings_status("Saved.".into());
}

fn append_log(window: &AppWindow, line: &str) {
    let mut log = window.get_export_log().to_string();
    if !log.is_empty() {
        log.push('\n');
    }
    log.push_str(line);
    window.set_export_log(log.into());
}

/// Build the argv list(s) to run and kick off the sequential runner
/// thread. Validates that an input was picked and at least one method is
/// checked, surfacing a clear error in the log/status instead of
/// spawning anything if not.
fn run_export(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    let (input, cfg) = {
        let s = state.borrow();
        (s.export_input.clone(), s.config.clone())
    };

    let Some(input) = input else {
        window.set_export_status("Error: pick a file, folder, or use the raw dir first.".into());
        return;
    };

    let do_csv = window.get_export_do_csv();
    let do_sqlite = window.get_export_do_sqlite();
    if !do_csv && !do_sqlite {
        window.set_export_status("Error: check at least one export method (CSV or SQLite).".into());
        return;
    }

    let sqlite_path = window.get_export_sqlite_path().to_string();
    let out_dir_override = window.get_export_out_dir().to_string();
    let base_args = input.base_args();

    let mut invocations: Vec<Vec<String>> = Vec::new();
    if do_csv {
        let mut args = base_args.clone();
        args.push("--csv".to_string());
        if !out_dir_override.is_empty() {
            args.push("--output-dir".to_string());
            args.push(out_dir_override.clone());
        }
        invocations.push(args);
    }
    if do_sqlite {
        let mut args = base_args.clone();
        args.push("--sqlite".to_string());
        if !sqlite_path.is_empty() {
            args.push(sqlite_path.clone());
        }
        invocations.push(args);
    }

    let binary = process::resolve_binary("acr_export.exe");
    // "Open output folder" should point at wherever CSV/LD actually landed:
    // the custom output dir if one is set, else the source-relative default.
    let output_dir = if out_dir_override.is_empty() {
        input.output_dir(&cfg).to_string_lossy().into_owned()
    } else {
        out_dir_override.clone()
    };

    window.set_export_running(true);
    window.set_export_status("Running…".into());
    window.set_export_log("".into());
    window.set_export_output_dir("".into());

    let window_weak = window.as_weak();
    std::thread::spawn(move || run_queue(window_weak, binary, invocations, output_dir));
}

/// Runs each `acr_export` invocation in `invocations` one after another
/// on this background thread, forwarding every stdout/stderr line to the
/// log panel via `slint::invoke_from_event_loop`, stopping at the first
/// failure (spawn error or nonzero/missing exit code).
fn run_queue(
    window_weak: Weak<AppWindow>,
    binary: PathBuf,
    invocations: Vec<Vec<String>>,
    output_dir: String,
) {
    let mut last_line = String::new();

    for args in &invocations {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

        let child = match process::spawn_hidden(&binary, &arg_refs) {
            Ok(child) => child,
            Err(e) => {
                let msg = format!("Failed to launch {}: {e}", binary.display());
                finish(&window_weak, false, msg, String::new());
                return;
            }
        };

        let (tx, rx) = mpsc::channel();
        process::stream_output(child, tx);

        let mut exit_code: Option<i32> = None;
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

        if exit_code != Some(0) {
            let msg = match exit_code {
                Some(code) => format!("acr_export exited with code {code}. Last output: {last_line}"),
                None => format!("acr_export exited abnormally. Last output: {last_line}"),
            };
            finish(&window_weak, false, msg, String::new());
            return;
        }
    }

    finish(&window_weak, true, "Export complete.".to_string(), output_dir);
}

fn finish(window_weak: &Weak<AppWindow>, success: bool, status: String, output_dir: String) {
    let window_weak = window_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window_weak.upgrade() {
            window.set_export_running(false);
            window.set_export_status(if success {
                status.into()
            } else {
                format!("Error: {status}").into()
            });
            if success {
                window.set_export_output_dir(output_dir.into());
            }
        }
    });
}
