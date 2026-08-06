//! Reusable child-process spawn/monitor helpers for launching one of
//! `acr_telemetry`'s CLI binaries (`acr_recorder.exe`, `acr_motec.exe`,
//! `acr_export.exe`, ...) without a console window popping up, and
//! streaming its stdout/stderr back into the GUI line-by-line.
//!
//! Deliberately generic — not specific to recording vs. exporting — so
//! both the (not-yet-built) recorder panel and export panel can share it.
//! See `docs/plans/acr-launcher-v1.md`'s "Recording panel"/"Export panel"
//! sections for how each is expected to use this.
//!
//! Unused for now (this unit only wires up the window shell + status
//! poll) — the recorder/export panel units are what call these.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;

/// Diagnostic logging that can't panic. Built with `windows_subsystem =
/// "windows"` (see `main.rs`), this app has no console attached, so
/// `eprintln!`'s `.expect("failed printing to stdout")` would abort the
/// process the first time a diagnostic line fires. Swallow the write
/// error instead — there's nowhere for the line to go, but that's not
/// worth crashing the GUI over.
pub fn log_err(msg: impl std::fmt::Display) {
    let _ = writeln!(std::io::stderr(), "{msg}");
}

/// Windows `CREATE_NO_WINDOW` process creation flag: suppresses the
/// console window that would otherwise flash up for a console
/// subprocess spawned from a GUI app. See
/// `CreateProcessW`/`STARTUPINFO` docs.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A line of output from a spawned child process, tagged by which stream
/// it came from so callers can decide how to display/filter each (e.g.
/// only stderr carries the recorder/exporter's progress lines today, but
/// both are forwarded so nothing is silently dropped).
#[derive(Debug, Clone)]
pub enum ChildOutput {
    Stdout(String),
    Stderr(String),
    /// The child process exited, with its exit code if one was available
    /// (a `None` exit status, e.g. terminated by a signal, has no
    /// portable equivalent on Windows — treated as `None` here too).
    Exited(Option<i32>),
}

/// The directory the launcher's own executable lives in — same
/// convention `acr_recorder::config::base_dir()` uses, so a binary
/// dropped next to `acr-launcher.exe` resolves the same way the
/// existing batch-file workflow expects (README already tells users to
/// keep all `acr_*.exe` together).
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// Resolve `binary_name` (e.g. `"acr_recorder.exe"`) against [`exe_dir`].
/// Falls back to the bare name (resolved via `PATH`) if the launcher's
/// own directory can't be determined.
pub fn resolve_binary(binary_name: &str) -> PathBuf {
    exe_dir()
        .map(|dir| dir.join(binary_name))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(binary_name))
}

/// Spawn `binary_path` with `args`, piping stdout/stderr and suppressing
/// the console window it would otherwise create. Callers own the
/// returned [`Child`] (for e.g. checking `try_wait()` or holding it
/// alive) — pair with [`stream_output`] to read its stdout/stderr on a
/// background thread.
pub fn spawn_hidden(
    binary_path: &Path,
    args: &[&str],
) -> std::io::Result<Child> {
    Command::new(binary_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
}

/// Take `child`'s stdout and stderr (if present) and forward each line —
/// plus a final [`ChildOutput::Exited`] once the process ends — to
/// `sender` from a single background thread. Silently does nothing with
/// lines once the receiving end is dropped (a closed channel just ends
/// the thread early via the `let _ =` below rather than panicking).
///
/// Blocks the *spawned* thread, not the caller: this function returns
/// immediately, leaving the reading/waiting to run in the background.
pub fn stream_output(mut child: Child, sender: Sender<ChildOutput>) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    std::thread::spawn(move || {
        // Read stderr on its own thread (recorder/export binaries print
        // their progress there — see src/main.rs's eprintln! calls, e.g.
        // "Recording to: ...", "Waiting for ACC shared memory...",
        // "Done. Recorded N samples...") while stdout is read inline
        // below, so neither pipe can back up and stall the child.
        let stderr_sender = sender.clone();
        let stderr_thread = stderr.map(|stderr| {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if stderr_sender.send(ChildOutput::Stderr(line)).is_err() {
                        return;
                    }
                }
            })
        });

        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(ChildOutput::Stdout(line)).is_err() {
                    break;
                }
            }
        }

        if let Some(t) = stderr_thread {
            let _ = t.join();
        }

        let exit_code = child.wait().ok().and_then(|status| status.code());
        let _ = sender.send(ChildOutput::Exited(exit_code));
    });
}

/// Whether a process named `image_name` (e.g. `"acr.exe"`) currently
/// appears in `tasklist`'s process list. Used for the Status tab's
/// "Launch AC Rally" button instead of the ACC-shared-memory poll —
/// overlay tools like SimHub keep the ACC-shaped shared memory segments
/// (`Local\acpmf_physics` etc.) alive on their own, so shared-memory
/// presence alone doesn't mean the game's own process is actually up.
pub fn is_process_running(image_name: &str) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {image_name}"), "/NH", "/FO", "CSV"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .to_lowercase()
            .contains(&image_name.to_lowercase()),
        Err(_) => false,
    }
}
