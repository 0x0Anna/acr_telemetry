# Graphics and statics (recording & SQLite)

This note aligns **documentation** with the code paths that write **rkyv sidecars** and **`acr_export --sqlite`** output.

## Source of truth

| What | Where |
|------|--------|
| Layout / Rust structs | `GraphicsRecord`, `StaticsRecord`, `PhysicsRecord` in `src/record.rs` |
| SQLite `CREATE TABLE` | `SCHEMA` in `src/export/sqlite_export.rs` (`graphics`, `statics`, `physics` tables) |
| ACC shared memory field meanings | `vendor/acc_shared_memory_rs/GRAPHICS_MAP.md`, `STATICS_MAP.md`, `PHYSICS_MAP.md` |

SQLite column names match the recorder structs (snake_case). One row of `statics` per recording; many rows of `graphics` at ~60 Hz when `record_graphics = true`; many rows of `physics` at the physics sample rate (~333 Hz).

## Graphics: what is stored

- **All fields** in `GraphicsRecord` are written to the `graphics` table (see `sqlite_export.rs`).
- **Car position:** `car_coordinates_x` / `y` / `z` are the **player** world position: `GraphicsRecord::from_graphics` selects the vector from `GraphicsMap.car_coordinates` whose slot matches `player_car_id` (`src/record.rs`).
- **Booleans in SQLite:** stored as `INTEGER` 0/1 in the insert path.

### Slots now carried through to SQLite (same ACC shared-memory layout as before)

- `replay_time_multiplier` (REAL) — replay-speed slot from `SPageFileGraphic`; often unused in live driving.
- `surface_grip` (REAL)
- `i_split` (INTEGER)

## Statics: what is stored

- One row per recording in `statics`, columns aligned with `StaticsRecord` / `StaticsMap`.
- **`track_spline_length` (REAL):** track spline length in metres (ACC static shared memory; aligns with e.g. SimHub `StaticInfo.TrackSplineLength`).

In ACC, `track`, `car_model`, player name fields, `max_rpm`, aids, pit window, tyre names, etc. are typically populated once the session is running. If recording starts before the sim finishes initializing, some strings may still be empty.

## Physics (SQLite only)

The bridge does not expose every physics column; the full set is in **`docs/FIELDS.md`** and in the `physics` table schema. One addition aligned with ACC shared memory:

- **`tyre_temp_extra_fl` / `_fr` / `_rl` / `_rr` (REAL):** fourth per-wheel temperature block (`PhysicsRecord.tyre_temp_extra` / `PhysicsMap.tyre_temp_extra`), same memory offset that older code skipped as a duplicate read.

## ACC vs AC Rally

Older notes referred to **AC Rally 0.2 / 0.3** where parts of `GraphicsMap` could look empty in captures. **Assetto Corsa Competizione** normally fills most graphics fields during a live session. Treat any “field X is always zero” claim as **game- and session-dependent** — re-run `scripts/compare_telemetry_docs_fields.py` or SQL `MIN`/`MAX` on your own `telemetry.db`.

## Schema changes and existing databases

`export_to_sqlite` and `export_graphics_to_sqlite` run **`ALTER TABLE … ADD COLUMN`** for new columns when opening the database (errors ignored if the column already exists), so **SQLite** files grow new columns without manual migration.

**Binary `.rkyv` physics/graphics blobs** still follow whatever `PhysicsRecord` / `GraphicsRecord` layout the recorder was built with. Older recordings are not rewritten; use a matching binary to read them, or re-record if you need new fields in rkyv.

`acr_analysis_export` applies the same `ALTER` pattern on **`main`** and **`src`** (attached telemetry DB) so `INSERT … SELECT *` copies stay column-compatible.

## Cross-check tools

- `python scripts/compare_telemetry_docs_fields.py --db path/to/telemetry.db` — physics vs `docs/FIELDS.md`, plus `graphics` / `statics` variability on **your** export.
- `cargo run --bin analyze_fields -- …` — physics-only constant/variable summary.

## Parity checklist

- **`physics`:** `CREATE TABLE … physics` in `sqlite_export.rs` ↔ `PhysicsRecord` (including `tyre_temp_extra_*`).
- **`graphics`:** ↔ `GraphicsRecord` fields (including `replay_time_multiplier`, `surface_grip`, `i_split`).
- **`statics`:** ↔ `StaticsRecord` fields (including `track_spline_length`).

The sidecar **`.json`** format description is emitted by the recorder from `src/format_meta.rs` and should track **`PhysicsRecord`** field order for the rkyv payload description.
