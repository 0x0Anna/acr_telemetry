
# Graphics and Statics Fields Analysis

Analysis of graphics and statics tables showing field variability and ranges.


**Variability:** Fields marked 'yes' contain varying data. Fields marked 'no' are constant or contain no useful data.
**Range:** Shows the range of values (for numeric fields) or sample values (for text fields).
**Note:** Zeros are excluded from min/max calculations (assumption: 0 = no data).

## Key Findings

**Graphics Table:** AC Rally 0.2 does **not populate** the graphics shared memory structure, it seems that has been carried over mostly into 0.3. A total of 80 out 84 fields in the graphics table contain only zeros or empty values. The exceptions since 0.3: car_coordinates_[x/y/z] and distance_traveled. Graphics are **recorded by default** (config: `record_graphics = true` in `acr_recorder.toml`); use `--no-graphics` to disable. For ACR v0.3, the graphics data will be recorded by acr_recorder by default.

**Statics Table:** Only the `car_model` field contains useful data (6 different car models recorded). All other static session information fields (track name, player info, session config, etc.) are empty or zero in AC Rally 0.2 and 0.3. It gets recorded as standard. Please note that the car_model might also not get detected if you start to record while the simulation has already started.

**Recommendation:** Focus on the **physics table** for all telemetry analysis. The physics table contains rich, high-frequency (333 Hz) data with many useful fields documented in [FIELDS.md](FIELDS.md).

---

## Graphics Fields

| Field | Description | Variable | Range |
|-------|-------------|----------|-------|
| `abs_level` | ABS level | no | constant 0 (no data) |
| `active_cars` | Number of active cars | no | constant 0 (no data) |
| `best_time` | Best lap time (ms) | no | constant 0 (no data) |
| `best_time_str` | Best lap time (formatted string) | no | empty/null |
| `car_coordinates_x` | Car world position X | yes | now showing local coordinates (not geographically referenced) in meters |
| `car_coordinates_y` | Car world position Y | yes | now showing local coordinates (not geographically referenced) in meters |
| `car_coordinates_z` | Car world position Z | yes | now showing local coordinates (not geographically referenced) in meters |
| `clock` | Session time (s) | no | constant 0 (no data) |
| `completed_lap` | Number of completed laps | no | constant 0 (no data) |
| `current_sector_index` | Current sector (0-based) | no | constant 0 (no data) |
| `current_time` | Current lap time (ms) | no | constant 07:54:19 |
| `current_time_str` | Current lap time (formatted string) | no | empty/null |
| `current_tyre_set` | Current tyre set | no | constant 0 (no data) |
| `delta_lap_time` | Delta to best lap (ms) | no | constant 0 (no data) |
| `delta_lap_time_str` | Delta to best lap (formatted string) | no | empty/null |
| `direction_light_left` | Left indicator on | no | constant 0 (no data) |
| `direction_light_right` | Right indicator on | no | constant 0 (no data) |
| `distance_traveled` | *Distance traveled (m) | no | now showing distance travelled in meters |
| `driver_stint_time_left` | Driver stint time left (s) | no | constant 0 (no data) |
| `driver_stint_total_time_left` | Driver stint total time left (s) | no | constant 0 (no data) |
| `engine_map` | Engine map setting | no | constant 0 (no data) |
| `estimated_lap_time` | Estimated lap time (ms) | no | constant 0 (no data) |
| `estimated_lap_time_str` | Estimated lap time (formatted string) | no | empty/null |
| `exhaust_temp` | Exhaust temperature (K) | no | constant 0 (no data) |
| `flag` | Flag status (0=none, 1=blue, 2=yellow, 3=black, 4=white, 5=checkered, 6=penalty) | no | constant 0 (no data) |
| `flashing_light` | Flashing light on | no | constant 0 (no data) |
| `fuel_estimated_laps` | Estimated laps remaining with current fuel | no | constant 0 (no data) |
| `fuel_per_lap` | Fuel consumption per lap (L) | no | constant 0 (no data) |
| `gap_ahead` | Gap to car ahead (ms) | no | constant 0 (no data) |
| `gap_behind` | Gap to car behind (ms) | no | constant 0 (no data) |
| `global_chequered` | Global checkered flag | no | constant 0 (no data) |
| `global_green` | Global green flag | no | constant 0 (no data) |
| `global_red` | Global red flag | no | constant 0 (no data) |
| `global_white` | Global white flag | no | constant 0 (no data) |
| `global_yellow` | Global yellow flag | no | constant 0 (no data) |
| `global_yellow_s1` | Yellow flag sector 1 | no | constant 0 (no data) |
| `global_yellow_s2` | Yellow flag sector 2 | no | constant 0 (no data) |
| `global_yellow_s3` | Yellow flag sector 3 | no | constant 0 (no data) |
| `ideal_line_on` | Ideal racing line enabled | no | constant 0 (no data) |
| `is_delta_positive` | Delta is positive (slower than best) | no | constant 0 (no data) |
| `is_in_pit` | Car is in pit box | no | constant 0 (no data) |
| `is_in_pit_lane` | Car is in pit lane | no | constant 0 (no data) |
| `is_setup_menu_visible` | Setup menu is visible | no | constant 0 (no data) |
| `is_valid_lap` | Current lap is valid | no | constant 0 (no data) |
| `last_sector_time` | Last sector time (ms) | no | constant 0 (no data) |
| `last_sector_time_str` | Last sector time (formatted string) | no | empty/null |
| `last_time` | Last lap time (ms) | no | constant 0 (no data) |
| `last_time_str` | Last lap time (formatted string) | no | empty/null |
| `light_stage` | Light stage | no | constant 0 (no data) |
| `main_display_index` | Main display page index | no | constant 0 (no data) |
| `mandatory_pit_done` | Mandatory pit stop completed | no | constant 0 (no data) |
| `mfd_fuel_to_add` | MFD fuel to add (L) | no | constant 0 (no data) |
| `mfd_tyre_pressure_fl` | MFD target tyre pressure FL (psi) | no | constant 0 (no data) |
| `mfd_tyre_pressure_fr` | MFD target tyre pressure FR (psi) | no | constant 0 (no data) |
| `mfd_tyre_pressure_rl` | MFD target tyre pressure RL (psi) | no | constant 0 (no data) |
| `mfd_tyre_pressure_rr` | MFD target tyre pressure RR (psi) | no | constant 0 (no data) |
| `mfd_tyre_set` | MFD tyre set selection | no | constant 0 (no data) |
| `missing_mandatory_pits` | Number of mandatory pits remaining | no | constant 0 (no data) |
| `normalized_car_position` | Position on track (0-1) | no | constant 0 (no data) |
| `number_of_laps` | Total number of laps in session | no | constant 0 (no data) |
| `penalty` | Penalty type | no | constant 0 (no data) |
| `penalty_time` | Penalty time (s) | no | constant 0 (no data) |
| `player_car_id` | Player car ID | no | constant 0 (no data) |
| `position` | Current position in race | no | constant 0 (no data) |
| `rain_intensity` | Current rain intensity | no | constant 0 (no data) |
| `rain_intensity_in_10min` | Rain intensity forecast +10min | no | constant 0 (no data) |
| `rain_intensity_in_30min` | Rain intensity forecast +30min | no | constant 0 (no data) |
| `rain_light` | Rain light on | no | constant 0 (no data) |
| `rain_tyres` | Rain tyres equipped | no | constant 0 (no data) |
| `secondary_display_index` | Secondary display page index | no | constant 0 (no data) |
| `session_index` | Current session index | no | constant 0 (no data) |
| `session_time_left` | Session time remaining (s) | no | constant 0 (no data) |
| `session_type` | Session type (0=unknown, 1=practice, 2=qualify, 3=race, etc.) | no | constant 0 (no data) |
| `status` | Session status (0=off, 1=replay, 2=live, 3=pause) | no | constant 0 (no data) |
| `strategy_tyre_set` | Strategy tyre set | no | constant 0 (no data) |
| `tc_cut_level` | TC cut level | no | constant 0 (no data) |
| `tc_level` | Traction control level | no | constant 0 (no data) |
| `track_grip_status` | Track grip status | no | constant 0 (no data) |
| `track_status` | Track status string | no | empty/null |
| `tyre_compound` | Tyre compound name | no | empty/null |
| `used_fuel` | Fuel used (L) | no | constant 0 (no data) |
| `wind_direction` | Wind direction (rad) | no | constant 0 (no data) |
| `wind_speed` | Wind speed (m/s) | no | constant 0 (no data) |
| `wiper_stage` | Wiper setting | no | constant 0 (no data) |

---

## Statics Fields

| Field | Description | Variable | Range |
|-------|-------------|----------|-------|
| `ac_version` | Assetto Corsa version | no | empty/null |
| `aid_auto_clutch` | Auto clutch enabled | no | constant 0 (no data) |
| `aid_fuel_rate` | Fuel consumption aid multiplier | no | constant 0 (no data) |
| `aid_mechanical_damage` | Mechanical damage aid multiplier | no | constant 0 (no data) |
| `aid_stability` | Stability aid level | no | constant 0 (no data) |
| `aid_tyre_rate` | Tyre wear aid multiplier | no | constant 0 (no data) |
| `car_model` | Car model name | yes | i.e. FIAT 131 Abarth, Hyundai i20N Rally2, Lancia 037, Citroen Xsara WRC, Lancia Stratos HF, Lancia Delta Integrale Evo... |
| `dry_tyres_name` | Dry tyres name | no | empty/null |
| `is_online` | Online session | no | constant 0 (no data) |
| `max_fuel` | Maximum fuel capacity (L) | no | constant 0 (no data) |
| `max_rpm` | Maximum RPM | no | constant 0 (no data) |
| `num_cars` | Number of cars | no | constant 0 (no data) |
| `number_of_sessions` | Number of sessions | no | constant 0 (no data) |
| `penalty_enabled` | Penalties enabled | no | constant 0 (no data) |
| `pit_window_end` | Pit window end lap | no | constant 0 (no data) |
| `pit_window_start` | Pit window start lap | no | constant 0 (no data) |
| `player_name` | Player first name | no | empty/null |
| `player_nick` | Player nickname | no | empty/null |
| `player_surname` | Player surname | no | empty/null |
| `sector_count` | Number of sectors | no | constant 0 (no data) |
| `sm_version` | Shared memory version | no | empty/null |
| `track` | Track name | no | empty/null |
| `wet_tyres_name` | Wet tyres name | no | empty/null |

---


## Summary

**Graphics:** 3/84 fields have variable data
**Statics:** 1/23 fields have variable data
