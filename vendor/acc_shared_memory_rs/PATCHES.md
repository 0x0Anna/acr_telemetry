# Modifications to acc_shared_memory_rs

This directory contains a modified version of [acc_shared_memory_rs](https://gitlab.com/ai-projects219/race-engineer/acc-shared-memory-rust) by Naresh Kumar. The following changes extend the physics map for AC Rally / ACC telemetry support.

## Modified Files

### `src/maps/physics_map.rs`

**Added fields** (not in upstream):

| Field | Type | Description |
|-------|------|-------------|
| `wheel_load` | Wheels | Wheel load (N) per wheel |
| `camber_rad` | Wheels | Camber angle in radians per wheel |
| `mz` | Wheels | Aligning moment per wheel |
| `fz` | Wheels | Vertical force per wheel |
| `my` | Wheels | Overturning moment per wheel |
| `tc_in_action` | bool | Traction control currently active |
| `abs_in_action` | bool | ABS currently active |

### `src/parsers/physics_parser.rs`

**Parser changes** to read the additional fields from the shared memory layout:

- Added `wheel_load` (Wheels) after `wheel_slip`
- Added `camber_rad` (Wheels) after `tyre_core_temp`
- Added `mz`, `fz`, `my` (Wheels) after `current_max_rpm`
- Added `tc_in_action` (i32 → bool) before `suspension_damage`
- Added `abs_in_action` (i32 → bool) after `tc_in_action`

The byte offsets and field order follow the SPageFilePhysics structure used by AC Rally / ACC.

### `src/maps/statics_map.rs` / `src/parsers/statics_parser.rs`

- Expose `track_spline_length` (`f32`, meters) on `StaticsMap` (previously read only to preserve layout, then discarded).

### `src/maps/graphics_map.rs` / `src/parsers/graphics_parser.rs`

- Expose `replay_time_multiplier`, `surface_grip`, `i_split` (previously read for layout only).

### `src/maps/physics_map.rs` / `src/parsers/physics_parser.rs`

- Expose `tyre_temp_extra` (`Wheels`) for the per-wheel block after `suspension_damage` (previously discarded).

## Upstream Version

Based on upstream `acc_shared_memory_rs` (version 0.8.0 or equivalent). The upstream project may have evolved; this fork is maintained for ACR Recorder compatibility.
