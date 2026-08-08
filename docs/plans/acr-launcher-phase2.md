# acr-launcher — Phase 2: MoTeC channel coverage, hotkey/HID bindings, Track Match tab

## Context

The v1 launcher (Status/Record/Export tabs, `crates/acr-launcher`) is done and merged
into `acr-launcher-v1`. This phase picks up three items the original plan explicitly
deferred as backlog, in the priority order the user set: (1) MoTeC LD channel coverage,
(2) hotkey/HID bindings for start/stop-type actions, (3) a launcher tab for
`acr_track_match` (chosen as the next tool to wrap, ahead of timing/bridge/grip-
estimator/plot/analysis-export).

## 1. MoTeC LD channel coverage

**Current shape** (confirmed by reading the code): `src/export/motec_ld.rs` is pure
binary serialization with no channel knowledge. The actual mapping is TOML profile
(`config/motec_profiles/{rbr,rally,all_data}.toml`) → `ChannelSource` enum
(`src/export/motec_profile.rs:58-122`, currently ~55 variants) → string parsed in
`ChannelSource::parse` (`:125-193`) → pulled from `PhysicsRecord`/`GraphicsRecord` in
one big `match` in `extract_channel` (`:302-553`). Adding a channel is a mechanical
4-touch-point change: enum variant, `parse` string case, `extract_channel` match arm,
profile TOML entries. No writer changes needed.

**Gap**: `PhysicsRecord` (`src/record.rs:52-160`) has ~85 fields; only ~55 are mapped.
Notably unmapped scalars that are commonly wanted in i2: `clutch`, `fuel`, `tc`, `abs`,
`turbo_boost`, `air_temp`, `road_temp`, `water_temp`, `heading`, `pitch`, `roll`. Also
unmapped: per-wheel `brake_pressure` and `slip_angle` (both `WheelsRecord`, same
`{front_left, front_right, rear_left, rear_right}` shape as the already-mapped
`suspension_travel`/`brake_temp`), `ride_height_front`/`ride_height_rear`, and
`car_damage` (`CarDamageRecord { front, rear, left, right, center }`).

**Plan**: add these ~26 channels (11 scalars, 8 per-wheel [brake_pressure ×4,
slip_angle ×4], 2 ride-height, 5 car-damage) following the exact existing pattern —
e.g. `ChannelSource::BrakePressureFl => records.iter().map(|r|
r.brake_pressure.front_left).collect()`, mirroring `SuspensionTravelFl` at
`motec_profile.rs:333`. Then add corresponding `[[channels]]` entries:
- `all_data.toml`: every new channel (it's the "everything implemented" dump profile).
- `rally.toml` / `rbr.toml`: only the ones with a natural ADL/RBR-style name (clutch,
  fuel, tc/abs indicators, temps, heading/pitch/roll, brake pressure) — skip the more
  obscure ones (car_damage, ride_height) unless the existing profiles already imply a
  slot for them. Use `scripts/compare_rally_workspace_channels.py`'s `RALLY_MAP` as a
  naming reference where it already lists a MoTeC-side name for one of these fields.

Explicitly out of scope for this pass: the graphics-sidecar-only fields (lap/session/
flag/weather/pit data in `GraphicsRecord`, ~70 more fields) — bigger and lower-value,
left as further backlog.

## 2. Hotkey / HID bindings

New module `crates/acr-launcher/src/hotkeys.rs` plus a new "Hotkeys" tab in
`ui/app.slint`. Two input sources, per the user's confirmed scope (both keyboard and
controller/button-box):

- **Keyboard**: `global-hotkey` crate (Tauri's, cross-platform OS-level registration —
  works even when the launcher window isn't focused, which matters since the game
  window will usually have focus during a session).
- **Controller / button box**: `gilrs` crate (polls joystick/gamepad state; most button
  boxes enumerate as HID gamepads, so this covers them without raw HID parsing).

**Design**: a small fixed action list for v1 — Start Recording, Stop Recording — bound
to the same Slint callbacks the Record tab's buttons already trigger
(`window.invoke_recorder_start()` / `invoke_recorder_stop()`, Slint's generated
programmatic-invoke methods for declared callbacks), so no duplication of
`recorder_panel.rs`'s start/stop logic. Bindings are captured via a "click to bind, then
press a key/button" UI flow and stored in a new small TOML file (e.g.
`acr_launcher_hotkeys.toml` next to the exe, using the same `toml::to_string_pretty`
round-trip approach `recorder_panel.rs` already uses for `acr_recorder.toml`). Two
background listeners (keyboard via `global-hotkey`'s event channel, controller via a
`gilrs::Gilrs` polling loop on its own thread) both post into the UI thread via
`slint::invoke_from_event_loop`, following the exact pattern already used in
`spawn_status_poll`/`export_panel.rs`/`recorder_panel.rs`.

**Known trade-off to document, not solve now**: if the game itself has exclusive raw
input capture on the same controller/button-box device, `gilrs` may not see button
presses while the game has focus — flag this in the tab's UI copy rather than trying to
solve device-sharing in this pass.

## 3. Track Match tab

`acr_track_match` (`src/bin/acr_track_match.rs` → `src/track_match_app.rs`, ~2900
lines) does live/offline geometry matching against reference tracks and, in `--live`
mode, runs the full sector-timing engine (RTSS overlay, sqlite timing DB, HTML export)
until stopped — it's long-running, not a one-shot batch tool like `acr_export`. CLI:
`acr_track_match --refs <files-or-dir> (--input FILE.rkyv | --live) [~35 more optional
flags, all overridable via acr_track_match.toml/acr_timing.toml/acr_pacenotes.toml]`.

**Stop mechanism gap found**: unlike `acr_recorder`, `track_match_app.rs` only reacts to
Ctrl+C (`static RUNNING: AtomicBool`, toggled by `ctrlc_handler()` at `:5869`, checked
in the `--live` loop at `:3187`) — there's no stop-file support. Since the launcher
spawns child processes hidden (`CREATE_NO_WINDOW`, no shared console), a real Ctrl+C
signal can't reach it, and hard-killing risks losing state. **Backend fix, mirroring
the acr_recorder convention**: add a stop-file check to the `while RUNNING.load(...)`
loop at `track_match_app.rs:3187` (same `resolve_stop_file_path`-style helper, reusing
`acr_recorder::config` conventions) so `RUNNING` also flips false when the stop file
appears — small, in-keeping-with-the-existing-project-pattern change, not a workaround.

**GUI panel** (`crates/acr-launcher/src/track_match_panel.rs`, new "Track Match" tab):
- Config: reference tracks path/dir (`rfd` folder or multi-file picker), a toggle for
  `--live` vs `--input <file>` mode (file picker shown when not live).
  A collapsed "advanced" section is *not* in scope for v1 — expose only `--refs` and
  the live/input choice; everything else stays TOML-driven per the tool's existing
  config-first design (matches how the Record tab left distance-reset tuning fields
  out of the UI too).
- Live mode: Start/Stop follow `recorder_panel.rs`'s exact shape (`process::
  spawn_hidden`/`stream_output`, Stop writes the (new) stop file, status pill from
  substring-matching stdout — exact substrings to pick after checking what
  `track_match_app.rs` actually logs on track-lock/session events, e.g. via its
  existing `eprintln!`/log lines).
- Offline `--input` mode: follows `export_panel.rs`'s fire-and-forget shape (spawn,
  stream to log, done — no persistent "running" state needed beyond the one process).

## Files touched

**MoTeC channels** (`acr_telemetry` repo root):
- `src/export/motec_profile.rs`: ~26 new `ChannelSource` variants + `parse` cases +
  `extract_channel` match arms.
- `config/motec_profiles/all_data.toml`, `rally.toml`, `rbr.toml`: new `[[channels]]`
  entries (mirror into `install/config/motec_profiles/` if that's a synced copy —
  confirm at implementation time whether it's a build artifact or hand-maintained).

**Hotkeys**:
- `crates/acr-launcher/Cargo.toml`: add `global-hotkey`, `gilrs`.
- `crates/acr-launcher/src/hotkeys.rs` (new).
- `crates/acr-launcher/src/main.rs`: register module, call `hotkeys::init(...)`.
- `crates/acr-launcher/ui/app.slint`: new Hotkeys tab.

**Track Match**:
- `src/track_match_app.rs`: stop-file polling in the `--live` loop.
- `crates/acr-launcher/src/track_match_panel.rs` (new).
- `crates/acr-launcher/src/main.rs`: register module, call `track_match_panel::init(...)`.
- `crates/acr-launcher/ui/app.slint`: new Track Match tab.

## Sequencing / parallelization

These three are largely independent (different files, no shared state), so they can be
built as three parallel units like v1's Units A–D — each in its own git worktree
branched off `acr-launcher-v1`, merged by hand afterward. The only shared-file risk is
`app.slint`/`main.rs` again (each unit adds one tab + one module registration) — same
merge pattern already used successfully for the Record/Export panels.

## Verification

1. `cargo build --release` (whole workspace) — confirms the MoTeC channel additions
   and the `track_match_app.rs` stop-file change compile and don't break existing bins.
2. Export a recording with the updated `all_data` profile; open the `.ld` in MoTeC i2
   (if available) or at minimum confirm `acr_export ... --csv` runs clean and the new
   channels appear in the LD channel list (can inspect via `motec_ld.rs`'s own
   structures or a hex/text check if i2 isn't on hand).
3. `cargo build -p acr-launcher`; run it, bind a keyboard key to Start/Stop, confirm it
   fires the same callback as clicking the button (watch the Record tab's status pill
   change). Bind a controller button similarly if a device is available; if not, note
   that gilrs-side verification is deferred to when hardware is available.
4. Run the Track Match tab in `--input` mode against a sample `.rkyv` + a reference
   track file, confirm scores print into the log. Run `--live` mode (requires the game
   running), confirm Start/Stop works via the new stop-file mechanism rather than a
   hard kill.

## Status (2026-08-07 review)

All three items are implemented and merged into `acr-launcher-v1`, confirmed against
current source:

1. **MoTeC channels**: `src/export/motec_profile.rs` has `Clutch`, `Fuel`, `Tc`/`Abs`
   (check exact names if needed), `TurboBoost`, `AirTemp`, `RoadTemp`, `WaterTemp`,
   `Heading`, `Pitch`, `Roll`, `BrakePressureFl/Fr/Rl/Rr`, `SlipAngleFl/Fr/Rl/Rr`,
   `RideHeightFront/Rear`, and `CarDamageFront/Rear/Left/Right/Center` — the full ~26-
   channel set this doc scoped, each with `parse`/`extract_channel` wiring. Profile TOML
   assignment across `all_data`/`rally`/`rbr` not re-verified line-by-line in this pass;
   spot-check before relying on a specific profile having a specific new channel.
2. **Hotkeys**: `crates/acr-launcher/src/hotkeys.rs` exists; later commits
   (`ff78148`, `7ee10c3`, `50923e3`) simplified to a single toggle binding per action
   (start/stop, plus a Track Match toggle) rather than the original two-binding design —
   an intentional simplification made during implementation, not a gap.
3. **Track Match tab**: `track_match_panel.rs` exists; `6efacd5` added the stop-file
   support this doc called out as a backend gap.

Phase 2 is done. Cross-phase backlog (comment-preserving TOML, structured status-JSON)
tracked in `acr-launcher-v1.md`'s status section.
