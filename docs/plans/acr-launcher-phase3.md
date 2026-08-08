# acr-launcher — Phase 3: Grip Estimator, Plot Recording, Telemetry Bridge tabs

Written after implementation (the three panel modules already shipped referencing
"phase 3" in their header doc comments with no phase-3 plan file to point at — this
closes that gap) rather than before it, so this reads as a record of what was built and
why, not a forward-looking plan. Same three tools v1's "out of scope" backlog named
(`acr_grip_estimator`, `acr_plot_recording`, `acr_telemetry_bridge`), picked up after
phase 2's Track Match tab.

## Context

v1 (Status/Record/Export) and phase 2 (MoTeC channel coverage, hotkeys, Track Match) are
both merged into `acr-launcher-v1`. This phase wraps the three remaining tools v1
explicitly deferred, in increasing order of lifecycle complexity: Grip Estimator and
Plot Recording are one-shot batch tools with no config file of their own (purely
CLI-flag-driven), while Telemetry Bridge is a long-running server like the Record/Track
Match tabs, needing the same spawn/Start/Stop shape.

## 1. Grip Estimator tab

Wraps `acr_grip_estimator.exe` (`src/bin/acr_grip_estimator.rs`), a one-shot tool that
scores tire grip/traction from an existing recording via two mutually exclusive input
modes: `--sqlite <path> [--recording-id <i64>]` or `--rkyv <path> [--track] [--car]`,
both taking shared `--early-sec`/`--correction-sec` flags. Output is CSV-formatted text
on stdout only (a single batch print at the end, not streamed progress) — the panel
captures stdout into a monospace results panel, same log-panel pattern as the Export tab.

The tool has no config file of its own, so `crates/acr-launcher/src/grip_estimator_panel.rs`
persists the last-used mode/paths itself in `acr_launcher.toml`'s new `[grip_estimator]`
table (`launcher_config.rs`'s `GripEstimatorUiConfig`), saved automatically on every Run
rather than via a separate "Save" button, since there's no other natural save point.

## 2. Plot Recording tab

Wraps `acr_plot_recording.exe` (`src/bin/acr_plot_recording.rs`), a one-shot tool that
reads a physics `.rkyv` file (deriving the sibling `{stem}.graphics.rkyv` itself — no
separate graphics arg) and writes a self-contained Plotly HTML plot. CLI is positional
only (`<physics.rkyv> [output.html]`), defaulting the output to `{stem}_plot.html` next
to the input if left blank — `plot_recording_panel.rs` mirrors that same default so the
"Open plot" button has something to point at without parsing the tool's stdout for the
path it chose.

No config file, purely positional-arg driven, so the panel persists only the last-used
input directory (`acr_launcher.toml`'s `[plot_recording]` table) — mirrors
`export_panel.rs`'s fire-and-forget shape (spawn, stream to log, done; no Start/Stop
lifecycle needed).

## 3. Telemetry Bridge tab

Wraps `acr_telemetry_bridge.exe` (`src/bin/acr_telemetry_bridge.rs`), a long-running
server that reads ACC/AC Rally shared memory and serves it over UDP and/or HTTP for a
phone/second-screen dashboard (`docs/BRIDGE.md`). It needs the sim already running and
connectable when started — no retry, since `ACCSharedMemory::new()` is called once at
startup and the process exits on `Err` if the shared memory isn't up yet.

**Stop mechanism gap found (same shape as phase 2's Track Match fix)**: the bridge only
reacted to Ctrl+C (`ctrlc::set_handler`), and the launcher spawns it hidden with no
shared console for a real Ctrl+C to reach. **Backend fix**: added `stop_file_path()` and
a once-a-second stop-file poll to the bridge's read loop in
`src/bin/acr_telemetry_bridge.rs`, mirroring `track_match_app.rs`'s convention exactly,
but under its own filename (`acr_telemetry_bridge_stop`, not `acr_track_match`'s) since
the bridge commonly runs alongside the recorder/track-match processes and a shared stop
file would stop all of them at once. `stop_file_path()` is `pub` so the launcher could
import it — instead `telemetry_bridge_panel.rs` duplicates the same path resolution,
since the bridge is a standalone `src/bin/` binary (not a `lib.rs` module like
`acr_recorder::track_match_app`) with nothing for the launcher to `use`. **Known
trade-off**: the two copies must be kept in sync by hand if the convention ever changes.

**GUI panel**: Start/Stop lifecycle mirrors `recorder_panel.rs` — write the UI's
rate/UDP/HTTP/unit settings into a fresh `acr_telemetry_bridge.toml` on every Start (so
the spawned process picks them up via its normal config load with no new CLI flags,
avoiding flag/TOML duplication), spawn hidden, stream output into a log + status pill,
Stop writes the new stop file. The UI's own last-used settings are persisted separately
in `acr_launcher.toml`'s `[telemetry_bridge]` table so they pre-fill the tab next launch,
independent of the `acr_telemetry_bridge.toml` that gets overwritten fresh every Start.
`dashboard_slots`/`telemetry_colors` are deliberately left out of the v1 tab (advanced,
TOML-only, same "config-first, don't expose everything" precedent as Track Match's scope).

## Files touched

- `src/bin/acr_telemetry_bridge.rs`: `stop_file_path()` + stop-file polling in the read loop.
- `crates/acr-launcher/src/grip_estimator_panel.rs` (new).
- `crates/acr-launcher/src/plot_recording_panel.rs` (new).
- `crates/acr-launcher/src/telemetry_bridge_panel.rs` (new).
- `crates/acr-launcher/src/launcher_config.rs`: `GripEstimatorUiConfig`,
  `PlotRecordingUiConfig`, `TelemetryBridgeUiConfig` tables.
- `crates/acr-launcher/src/main.rs`: register the three modules, call their `init(...)`.
- `crates/acr-launcher/ui/app.slint`: three new tabs.
- Packaging (`install/build.ps1`, `install/ACR_Recorder.iss`, `install/PACKAGE_README.txt`,
  `install/README.md`, `.github/workflows/release.yml`): `acr_grip_estimator` and
  `acr_plot_recording` added to the release build/staging/installer lists — found missing
  during review (both binaries existed and were wrapped by the new tabs, but weren't
  being built or packaged for release).

## Verification

1. `cargo build --release` (whole workspace) — confirms the `acr_telemetry_bridge.rs`
   stop-file addition and the new panel modules compile without breaking existing bins.
2. Grip Estimator: run against a sample `.rkyv` (and separately a sample sqlite DB),
   confirm CSV rows appear in the results panel and the mode/paths pre-fill on relaunch.
3. Plot Recording: run against a sample `.rkyv`, confirm the HTML plot is written next to
   the input and "Open plot" opens it.
4. Telemetry Bridge: with the game running, Start, confirm the status pill flips to
   "Running" and `docs/BRIDGE.md`'s HTTP/UDP contract is being served; Stop, confirm the
   process exits via the stop file rather than needing to be killed.
5. `install/build.ps1` (or the CI workflow) — confirms `acr_grip_estimator.exe` and
   `acr_plot_recording.exe` are present in both the portable zip and the Inno Setup
   install directory.

## Status (2026-08-07 review)

All three tabs, the bridge stop-file fix, and the packaging updates are implemented and
merged into `acr-launcher-v1` (`abdc789` added the tabs + status/persistence work,
`d84d06a`/`f3b721a`/`be65805` are post-merge hardening on top). Confirmed by reading
current source, not just commit messages:

- `crates/acr-launcher/src/{grip_estimator_panel,plot_recording_panel,
  telemetry_bridge_panel}.rs` all exist; `launcher_config.rs` carries their UI-state
  tables (now consolidated into `acr_launcher.toml` per `b634ca8`, superseding this
  doc's earlier "own `[section]` table" framing — same idea, one file).
- `install/build.ps1`, `install/ACR_Recorder.iss`, and `.github/workflows/release.yml`
  all reference `acr_grip_estimator`/`acr_plot_recording` — packaging gap closed.
- `cargo build -p acr_launcher` is clean as of this review.

Phase 3 is done. No open items from this doc remain — see `acr-launcher-v1.md`'s status
section for the cross-phase backlog that's still open (comment-preserving TOML edits,
structured status-JSON, further tool tabs).
