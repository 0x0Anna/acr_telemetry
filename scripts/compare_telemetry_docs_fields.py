#!/usr/bin/env python3
"""
Compare docs/FIELDS.md expectations (Variable column) with empirical variability
in telemetry.db (physics), and summarize graphics + statics tables.

Usage:
  python scripts/compare_telemetry_docs_fields.py [path/to/telemetry.db]
  python scripts/compare_telemetry_docs_fields.py --db c:/temp/acc/telemetry.db --sample 50000

Does not use timing/timing.db (no physics table). Pass the SQLite file produced
by acr_export / acr_recorder (telemetry.db).

**"Variable" (default):** more than one distinct *signal* value in the sample.
SQL NULL and empty TEXT are not signal. For physics numerics, **0** (and |x|<1e-9)
counts as "no signal" except on channels where zero is a normal driving value
(gas/brake/clutch/gear/speed/temperatures in K, forces, wheel channels, booleans,
etc.). ``--legacy-variability`` uses the old rule: distinct over all non-NULL
values (zeros included).
"""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
FIELDS_MD = REPO_ROOT / "docs" / "FIELDS.md"

_PHYSICS_ZERO_IS_SIGNAL = frozenset(
    {
        "gas",
        "brake",
        "clutch",
        "gear",
        "steer_angle",
        "speed_kmh",
        "packet_id",
        "heading",
        "pitch",
        "roll",
        "final_ff",
        "turbo_boost",
        "tc",
        "abs",
        "brake_bias",
        "fuel",
        "water_temp",
        "road_temp",
        "air_temp",
        "current_max_rpm",
        "number_of_tyres_out",
        "pit_limiter_on",
        "engine_brake",
    }
)
_PHYSICS_ZERO_SIGNAL_PREFIXES = (
    "velocity_",
    "local_velocity_",
    "local_angular_vel_",
    "g_force_",
    "wheel_slip_",
    "wheel_load_",
    "wheel_pressure_",
    "wheel_angular_speed_",
    "slip_ratio_",
    "slip_angle_",
    "mz_",
    "fz_",
    "my_",
    "tyre_core_temp_",
    "brake_temp_",
    "suspension_travel_",
    "tyre_contact_",
)


def physics_zero_counts_as_signal(col: str) -> bool:
    """If True, numeric 0 is a real telemetry value for variability."""
    if col in _PHYSICS_ZERO_IS_SIGNAL:
        return True
    if col.startswith(_PHYSICS_ZERO_SIGNAL_PREFIXES):
        return True
    if col.endswith("_on") or col.endswith("_in_action") or col.startswith("is_"):
        return True
    if col.startswith(("drs", "p2p", "ers_", "kers_", "front_brake_compound", "rear_brake_compound")):
        return True
    return False


def pragma_coltypes(conn: sqlite3.Connection, table: str) -> dict[str, str]:
    cur = conn.execute(f"PRAGMA table_info({table})")
    return {row[1]: (row[2] or "").upper() for row in cur.fetchall()}


def signal_predicate_on_alias(
    col: str,
    col_type: str,
    *,
    table: str,
    legacy_variability: bool,
    value_alias: str = "v",
) -> str:
    """Predicate on a subquery column alias (e.g. ``v``), for capped raw scans."""
    c = value_alias
    if legacy_variability:
        return f"({c} IS NOT NULL)"

    if col_type == "TEXT":
        return f"({c} IS NOT NULL AND TRIM(CAST({c} AS TEXT)) != '')"

    if table == "graphics":
        return f"({c} IS NOT NULL)"

    if table == "statics":
        if col_type == "TEXT":
            return f"({c} IS NOT NULL AND TRIM(CAST({c} AS TEXT)) != '')"
        if col in (
            "number_of_sessions",
            "num_cars",
            "sector_count",
            "max_rpm",
            "max_fuel",
            "penalty_enabled",
            "pit_window_start",
            "pit_window_end",
            "is_online",
            "aid_auto_clutch",
        ) or col.startswith("aid_"):
            return f"({c} IS NOT NULL AND ABS(CAST({c} AS REAL)) > 1e-9)"
        return f"({c} IS NOT NULL)"

    if physics_zero_counts_as_signal(col):
        return f"({c} IS NOT NULL)"
    return f"({c} IS NOT NULL AND ABS(CAST({c} AS REAL)) > 1e-9)"


def signal_where_sql(
    col: str,
    col_type: str,
    *,
    table: str,
    legacy_variability: bool,
    table_alias: str = "t",
) -> str:
    """SQL predicate on ``{table_alias}.{col}`` (same rules as ``signal_predicate_on_alias``)."""
    c = f"{table_alias}.{col}"
    return signal_predicate_on_alias(
        col, col_type, table=table, legacy_variability=legacy_variability, value_alias=c
    )


def parse_fields_md_expectations(path: Path) -> dict[str, str]:
    """Map physics column name -> Variable cell: 'yes', 'varies', '—', etc."""
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    expectations: dict[str, str] = {}
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.strip() == "## Global Fields":
            i += 1
            while i < len(lines):
                row = lines[i]
                if row.strip().startswith("## ") or row.strip() == "---":
                    break
                parts = [p.strip() for p in row.split("|")]
                if len(parts) < 5:
                    i += 1
                    continue
                field_cell = parts[1]
                variable_cell = parts[3]
                if field_cell in ("Field", "") or set(field_cell) <= {"-", " "}:
                    i += 1
                    continue
                names = re.findall(r"`([^`]+)`", field_cell)
                if not names:
                    i += 1
                    continue
                v = variable_cell.replace("…", "").strip()
                expanded: list[str] = []
                for n in names:
                    if re.match(r"^[a-z0-9_]+_x/y/z$", n, re.I):
                        stem = n[: -len("_x/y/z")]
                        expanded.extend(f"{stem}_{a}" for a in ("x", "y", "z"))
                    else:
                        expanded.append(n)
                for n in expanded:
                    expectations[n] = v
                i += 1
            break
        i += 1

    # Per-wheel bases: doc table has no Variable column — treat all wheel channels as driving-varying.
    i = 0
    wheel_bases: list[str] = []
    while i < len(lines):
        if lines[i].strip() == "## Per-Wheel (`_fl`, `_fr`, `_rl`, `_rr`)":
            i += 1
            while i < len(lines):
                row = lines[i]
                if row.strip().startswith("## ") or row.strip().startswith("**Tyre contact"):
                    break
                parts = [p.strip() for p in row.split("|")]
                if len(parts) >= 4:
                    base_cell = parts[1]
                    if base_cell not in ("Base", "") and not base_cell.startswith("-"):
                        for name in re.findall(r"`([^`]+)`", base_cell):
                            wheel_bases.append(name)
                i += 1
            break
        i += 1

    # De-duplicate wheel bases while keeping order
    seen_w: set[str] = set()
    wheel_bases = [b for b in wheel_bases if not (b in seen_w or seen_w.add(b))]

    suffixes = ("_fl", "_fr", "_rl", "_rr")
    for base in wheel_bases:
        for suf in suffixes:
            expectations[f"{base}{suf}"] = "yes"

    return expectations


def _emdash_var(v: str) -> bool:
    return v in ("—", "\u2014", "-") or v.startswith("\u2014")


def expects_motion_variable(v: str) -> bool:
    """Doc says the value should change during a lap / session (physics JSON or SQLite)."""
    if not v:
        return False
    if _emdash_var(v):
        return False
    vl = v.lower()
    return vl in ("yes", "varies") or vl.startswith("yes") or vl.startswith("varies")


def pragma_columns(conn: sqlite3.Connection, table: str) -> list[str]:
    cur = conn.execute(f"PRAGMA table_info({table})")
    return [row[1] for row in cur.fetchall()]


def column_stats_sample(
    conn: sqlite3.Connection,
    table: str,
    col: str,
    col_type: str,
    sample: int,
    skip: frozenset[str],
    *,
    spread_recording_ids: list[int] | None,
    legacy_variability: bool = False,
    raw_scan_cap: int = 120_000,
) -> dict[str, Any] | None:
    if col in skip:
        return None
    sig_v = signal_predicate_on_alias(
        col, col_type, table=table, legacy_variability=legacy_variability, value_alias="v"
    )
    if spread_recording_ids:
        ph = ",".join("?" * len(spread_recording_ids))
        q = f"""
        SELECT
          COUNT(*) AS n,
          COUNT(DISTINCT v) AS distinct_v,
          MIN(v) AS vmin,
          MAX(v) AS vmax
        FROM (
          SELECT v
          FROM (
            SELECT t.{col} AS v
            FROM {table} t
            WHERE t.recording_id IN ({ph})
            LIMIT ?
          ) raw
          WHERE {sig_v}
          LIMIT ?
        ) s
        """
        try:
            row = conn.execute(q, (*spread_recording_ids, raw_scan_cap, sample)).fetchone()
        except sqlite3.Error:
            return None
        if row is not None:
            n, distinct_v, vmin, vmax = row
            return {
                "n": int(n or 0),
                "distinct_v": int(distinct_v or 0),
                "vmin": vmin,
                "vmax": vmax,
                "variable": int(distinct_v or 0) > 1,
            }
        return None

    q = f"""
    SELECT
      COUNT(*) AS n,
      COUNT(DISTINCT v) AS distinct_v,
      MIN(v) AS vmin,
      MAX(v) AS vmax
    FROM (
      SELECT v
      FROM (
        SELECT t.{col} AS v
        FROM {table} t
        LIMIT ?
      ) raw
      WHERE {sig_v}
      LIMIT ?
    ) s
    """
    try:
        row = conn.execute(q, (raw_scan_cap, sample)).fetchone()
    except sqlite3.Error:
        return None
    if row is None:
        return {"n": 0, "distinct_v": 0, "vmin": None, "vmax": None, "variable": False}
    n, distinct_v, vmin, vmax = row
    return {
        "n": int(n or 0),
        "distinct_v": int(distinct_v or 0),
        "vmin": vmin,
        "vmax": vmax,
        "variable": int(distinct_v or 0) > 1,
    }


def statics_cross_session_stats(
    conn: sqlite3.Connection,
    col: str,
    col_type: str,
    skip: frozenset[str],
    *,
    total_rows: int,
    legacy_variability: bool = False,
) -> dict[str, Any] | None:
    if col in skip:
        return None
    sig = signal_where_sql(
        col,
        col_type,
        table="statics",
        legacy_variability=legacy_variability,
        table_alias="s",
    )
    q = f"""
    SELECT
      SUM(CASE WHEN {sig} THEN 1 ELSE 0 END),
      COUNT(DISTINCT CASE WHEN {sig} THEN s.{col} END),
      MIN(CASE WHEN {sig} THEN s.{col} END),
      MAX(CASE WHEN {sig} THEN s.{col} END)
    FROM statics s
    """
    try:
        row = conn.execute(q).fetchone()
    except sqlite3.Error:
        return None
    if not row:
        return None
    n, dv, vmin, vmax = row
    return {
        "rows": total_rows,
        "n_signal": n,
        "distinct_v": dv,
        "vmin": vmin,
        "vmax": vmax,
        "variable_across_recordings": dv > 1,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "db",
        nargs="?",
        default=None,
        help="Path to telemetry.db (default: <repo>/telemetry.db)",
    )
    ap.add_argument("--db", dest="db_opt", help="Explicit DB path (overrides positional)")
    ap.add_argument("--sample", type=int, default=15_000, help="Max signal rows per physics/graphics column")
    ap.add_argument(
        "--spread-recordings",
        type=int,
        default=15,
        help="Sample physics/graphics only from this many random recordings (0 = first rows only, faster but biased)",
    )
    ap.add_argument(
        "--raw-scan-cap",
        type=int,
        default=120_000,
        help="Max raw rows read per column before applying signal filter (limits full-table scans)",
    )
    ap.add_argument(
        "--legacy-variability",
        action="store_true",
        help="Count distinct on all non-NULL values (zeros count). Default: NULL/empty text and stripped zeros on physics.",
    )
    ap.add_argument("--json", action="store_true", help="Print machine-readable JSON only")
    args = ap.parse_args()
    db_path = Path(args.db_opt or args.db or (REPO_ROOT / "telemetry.db"))
    if not db_path.is_file():
        print(f"Database not found: {db_path}", file=sys.stderr)
        return 1

    if not FIELDS_MD.is_file():
        print(f"Missing {FIELDS_MD}", file=sys.stderr)
        return 1

    expectations = parse_fields_md_expectations(FIELDS_MD)
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    if not args.json:
        print(f"Analyzing {db_path} ...", flush=True)

    skip_phys = frozenset({"recording_id", "time_offset"})
    skip_graph = frozenset({"recording_id", "time_offset"})
    skip_static = frozenset({"recording_id"})

    physics_cols = pragma_columns(conn, "physics")
    phys_types = pragma_coltypes(conn, "physics")
    has_graphics = bool(
        conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='graphics'"
        ).fetchone()
    )
    has_statics = bool(
        conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='statics'"
        ).fetchone()
    )

    phys_total = conn.execute("SELECT COUNT(*) FROM physics").fetchone()[0]
    n_rec_phys = conn.execute("SELECT COUNT(DISTINCT recording_id) FROM physics").fetchone()[0]

    spread_ids: list[int] | None = None
    if args.spread_recordings > 0:
        spread_ids = [
            int(r[0])
            for r in conn.execute(
                "SELECT id FROM recordings ORDER BY RANDOM() LIMIT ?",
                (args.spread_recordings,),
            ).fetchall()
        ]

    report: dict[str, Any] = {
        "database": str(db_path.resolve()),
        "physics_rows": phys_total,
        "physics_recordings": n_rec_phys,
        "sample_limit": args.sample,
        "spread_recordings": args.spread_recordings,
        "spread_recording_ids_sample": (spread_ids[:5] if spread_ids else []),
        "raw_scan_cap": args.raw_scan_cap,
        "legacy_variability": args.legacy_variability,
        "physics_mismatches": [],
        "physics_doc_missing_in_db": [],
        "physics_db_not_in_doc": [],
        "physics_variable_not_in_fields_table": [],
        "physics_doc_variable_but_no_signal_rows": [],
        "physics_doc_not_variable_but_varies": [],
        "graphics": {},
        "statics": {},
    }

    # --- Physics vs FIELDS.md ---
    for k in sorted(expectations):
        if k not in physics_cols and not k.startswith("tyre_contact"):
            # tyre_contact_* may still be missing on very old DBs
            report["physics_doc_missing_in_db"].append(k)

    for col in physics_cols:
        if col in skip_phys:
            continue
        st = column_stats_sample(
            conn,
            "physics",
            col,
            phys_types.get(col, ""),
            args.sample,
            skip_phys,
            spread_recording_ids=spread_ids,
            legacy_variability=args.legacy_variability,
            raw_scan_cap=args.raw_scan_cap,
        )
        if not st or st["n"] == 0:
            variable = False
            st = st or {"n": 0, "distinct_v": 0, "variable": False}
        else:
            variable = st["variable"]

        exp = expectations.get(col)
        if exp is None:
            report["physics_db_not_in_doc"].append(
                {"column": col, "distinct_sample": st.get("distinct_v"), "variable": variable}
            )
            if variable:
                report["physics_variable_not_in_fields_table"].append(col)
        else:
            if expects_motion_variable(exp) and st["n"] == 0:
                report["physics_doc_variable_but_no_signal_rows"].append(
                    {"column": col, "doc_variable_cell": exp}
                )
            elif expects_motion_variable(exp) and not variable and st.get("n", 0) > 100:
                report["physics_mismatches"].append(
                    {
                        "column": col,
                        "issue": "doc_expects_variable_but_sample_constant_or_single_value",
                        "doc_variable_cell": exp,
                        "n": st.get("n"),
                        "distinct_v": st.get("distinct_v"),
                        "vmin": st.get("vmin"),
                        "vmax": st.get("vmax"),
                    }
                )
            elif (
                not expects_motion_variable(exp)
                and variable
                and st.get("n", 0) > 100
            ):
                report["physics_doc_not_variable_but_varies"].append(
                    {
                        "column": col,
                        "doc_variable_cell": exp,
                        "n": st.get("n"),
                        "distinct_v": st.get("distinct_v"),
                        "vmin": st.get("vmin"),
                        "vmax": st.get("vmax"),
                    }
                )

    # --- Graphics (GRAPHICS_STATICS_FIELDS.md has no per-column variability) ---
    if has_graphics:
        gcols = pragma_columns(conn, "graphics")
        graph_types = pragma_coltypes(conn, "graphics")
        g_total = conn.execute("SELECT COUNT(*) FROM graphics").fetchone()[0]
        g_variable: list[str] = []
        g_constant: list[str] = []
        g_low_card: list[tuple[str, int]] = []
        for col in gcols:
            if col in skip_graph:
                continue
            st = column_stats_sample(
                conn,
                "graphics",
                col,
                graph_types.get(col, ""),
                args.sample,
                skip_graph,
                spread_recording_ids=spread_ids,
                legacy_variability=args.legacy_variability,
                raw_scan_cap=args.raw_scan_cap,
            )
            if not st or st["n"] == 0:
                g_constant.append(f"{col} (all NULL in sample)")
                continue
            if st["variable"]:
                g_variable.append(col)
            else:
                g_constant.append(col)
            g_low_card.append((col, st["distinct_v"]))

        g_low_card.sort(key=lambda x: -x[1])
        report["graphics"] = {
            "rows": g_total,
            "columns": len(gcols),
            "variable_in_sample": sorted(g_variable),
            "constant_in_sample": sorted(g_constant),
            "top_distinct": [{"column": c, "distinct": d} for c, d in g_low_card[:25]],
        }

    # --- Statics (one row per recording): variability = across recordings ---
    if has_statics:
        scols = pragma_columns(conn, "statics")
        stat_types = pragma_coltypes(conn, "statics")
        statics_total = int(conn.execute("SELECT COUNT(*) FROM statics").fetchone()[0] or 0)
        statics_report: dict[str, Any] = {"columns": {}, "constant_across_all_recordings": []}
        for col in scols:
            if col in skip_static:
                continue
            st = statics_cross_session_stats(
                conn,
                col,
                stat_types.get(col, ""),
                skip_static,
                total_rows=statics_total,
                legacy_variability=args.legacy_variability,
            )
            if not st:
                continue
            statics_report["columns"][col] = st
            if st["rows"] > 1 and not st["variable_across_recordings"]:
                statics_report["constant_across_all_recordings"].append(col)
        report["statics"] = statics_report

    conn.close()

    if args.json:
        print(json.dumps(report, indent=2, default=str))
        return 0

    print(f"Database: {report['database']}")
    mode = "legacy (distinct over non-NULL, zeros count)" if args.legacy_variability else (
        "default (signal rows: non-NULL, non-empty text; physics zeros stripped except driving channels)"
    )
    print(f"Variability mode: {mode}\n")
    print(
        f"physics: {report['physics_rows']} rows, {report['physics_recordings']} recordings "
        f"(sample up to {args.sample} signal rows per column; "
        f"spread across {len(spread_ids) if spread_ids else 0} random recordings)\n"
    )

    nop = report.get("physics_doc_variable_but_no_signal_rows") or []
    if nop:
        print(
            f"=== Doc yes/varies but no signal rows in sample ({len(nop)}) ===\n"
            "(all NULL, empty, or stripped as zero on physics - not counted as variability.)\n"
        )
        for x in sorted(nop, key=lambda z: z["column"])[:50]:
            print(f"  - {x['column']}: doc={x['doc_variable_cell']!r}")
        if len(nop) > 50:
            print(f"  ... and {len(nop) - 50} more")
        print()

    print("=== FIELDS.md vs physics (expectation: Variable = yes|varies) ===\n")
    mm = report["physics_mismatches"]
    if not mm:
        print("No mismatches: every documented yes/varies column has >1 distinct *signal* value in the sample.\n")
    else:
        print(f"Mismatches ({len(mm)}): doc expects variability, sample shows one distinct signal value.\n")
        for m in sorted(mm, key=lambda x: x["column"]):
            print(
                f"  - {m['column']}: doc={m['doc_variable_cell']!r} "
                f"distinct={m['distinct_v']} n={m['n']} min={m['vmin']} max={m['vmax']}"
            )
        print()

    inv = report.get("physics_doc_not_variable_but_varies") or []
    print(
        "=== FIELDS.md: Variable is NOT yes/varies (e.g. em dash), but sample IS variable ===\n"
    )
    if not inv:
        print(
            "None: every column marked with '-' / non-yes|varies in the Global table is "
            "single-value or low-signal in this telemetry.db sample.\n"
        )
    else:
        print(
            f"Found {len(inv)} (doc says 'not for bridge variability' but physics data varies):\n"
        )
        for m in sorted(inv, key=lambda x: x["column"]):
            print(
                f"  - {m['column']}: doc Variable cell={m['doc_variable_cell']!r} "
                f"distinct_signal={m['distinct_v']} n_signal={m['n']} min={m['vmin']} max={m['vmax']}"
            )
        print()

    missing = [x for x in report["physics_doc_missing_in_db"] if not x.endswith(("_fl", "_fr", "_rl", "_rr"))]
    # Filter noise: wheel expansion creates keys not in DB if typo - actually all should exist
    if missing:
        print(f"=== Doc field names not in physics table ({len(missing)}) ===\n")
        for x in sorted(missing)[:40]:
            print(f"  - {x}")
        if len(missing) > 40:
            print(f"  ... and {len(missing) - 40} more")
        print()

    ndoc = report["physics_db_not_in_doc"]
    if ndoc:
        only_var = [x for x in ndoc if x["variable"]]
        print(
            f"=== physics columns not listed in FIELDS.md Global/Per-Wheel tables ({len(ndoc)}) ===\n"
            f"(expected for tyre_contact_* and any newer schema columns.)\n"
        )
        if only_var:
            print(f"Among them, variable in sample ({len(only_var)}):\n")
            for x in sorted(only_var, key=lambda z: z["column"]):
                print(f"  - {x['column']}: distinct={x['distinct_sample']}")
        print()

    if report.get("graphics"):
        g = report["graphics"]
        print("=== graphics (see docs/GRAPHICS_STATICS_FIELDS.md - no per-field Variable table) ===\n")
        print(f"rows={g['rows']}, columns={g['columns']}")
        print(f"variable in sample: {len(g['variable_in_sample'])}")
        print(f"constant / single-value in sample: {len(g['constant_in_sample'])}")
        print("\nTop columns by distinct count (sample):")
        for row in g.get("top_distinct", [])[:20]:
            print(f"  {row['column']}: {row['distinct']}")
        print("\nConstant in sample (may still be OK: e.g. unused flags):")
        for name in g["constant_in_sample"][:35]:
            print(f"  {name}")
        if len(g["constant_in_sample"]) > 35:
            print(f"  ... and {len(g['constant_in_sample']) - 35} more")
        print()

    if report.get("statics") and report["statics"].get("columns"):
        s = report["statics"]
        print("=== statics (variability = across recordings in this DB) ===\n")
        const = s.get("constant_across_all_recordings", [])
        if const:
            print(
                f"Same value on every statics row ({len(const)} cols) - normal if one car/track/settings:\n"
            )
            for c in sorted(const):
                info = s["columns"].get(c, {})
                print(f"  {c}: distinct={info.get('distinct_v')} min/max={info.get('vmin')!r}/{info.get('vmax')!r}")
        else:
            print("Every column takes >1 distinct value across statics rows.\n")

        print("Per-column distinct (all statics rows):")
        for col in sorted(s["columns"]):
            inf = s["columns"][col]
        print(
            f"  {col}: rows={inf['rows']} n_signal={inf.get('n_signal', inf['rows'])} "
            f"distinct={inf['distinct_v']} variable_across_recordings={inf['variable_across_recordings']}"
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
