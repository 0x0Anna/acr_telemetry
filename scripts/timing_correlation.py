#!/usr/bin/env python3
"""Correlate sector timing factors with duration and exit speed.

Reads sector_splits from timing.db (columns on the row: exit_speed_kmh, entry_speed_kmh,
throttle_open_pct, slip stats, …). Drops attempts slower than best * (1 + slow_pct/100)
per leg, computes Pearson r vs duration_sec and vs exit_speed_kmh, writes timing_factors.

Run from acc-stage-timing (default DB: timing/timing.db):

  python scripts/timing_correlation.py \\
      --track cwmbiga_afon_biga --car "Toyota GR Yaris Rally2" \\
      --direction stage --from-sector 0 --to-sector 4
"""

from __future__ import annotations

import argparse
import math
import sqlite3
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Sequence

STAGE_ROOT = Path(__file__).resolve().parent.parent

# Columns on sector_splits (acc-stage-timing / acr_track_match leg stats)
SPLIT_METRIC_COLUMNS = (
    "distance_m",
    "throttle_open_pct",
    "max_slip_angle",
    "max_slip_ratio",
    "min_slip_ratio",
    "entry_speed_kmh",
    "exit_speed_kmh",
)

# Prefer exit_speed_kmh (DB name); accept legacy end_speed_kmh if present
SPEED_TARGET_NAMES = ("exit_speed_kmh", "end_speed_kmh")

TIMING_FACTORS_DDL = """
CREATE TABLE IF NOT EXISTS timing_factors (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    computed_at_utc TEXT NOT NULL,
    track_name TEXT NOT NULL,
    car_model TEXT NOT NULL,
    direction TEXT NOT NULL,
    from_sector INTEGER NOT NULL,
    to_sector INTEGER NOT NULL,
    target TEXT NOT NULL,
    factor TEXT NOT NULL,
    correlation REAL,
    n_samples INTEGER NOT NULL,
    n_total INTEGER NOT NULL,
    best_duration_sec REAL,
    max_duration_sec REAL,
    UNIQUE (
        track_name, car_model, direction, from_sector, to_sector, target, factor
    )
);
"""


@dataclass(frozen=True)
class LegKey:
    track_name: str
    car_model: str
    direction: str
    from_sector: int
    to_sector: int


@dataclass
class Attempt:
    duration_sec: float
    values: dict[str, float]


def pearson(xs: Sequence[float], ys: Sequence[float]) -> float | None:
    n = len(xs)
    if n < 3 or n != len(ys):
        return None
    mx = sum(xs) / n
    my = sum(ys) / n
    num = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    den_x = math.sqrt(sum((x - mx) ** 2 for x in xs))
    den_y = math.sqrt(sum((y - my) ** 2 for y in ys))
    if den_x == 0.0 or den_y == 0.0:
        return None
    return num / (den_x * den_y)


def finite(v: Any) -> float | None:
    if v is None:
        return None
    try:
        f = float(v)
    except (TypeError, ValueError):
        return None
    if not math.isfinite(f):
        return None
    return f


def table_columns(conn: sqlite3.Connection, table: str) -> list[str]:
    return [
        str(row[1])
        for row in conn.execute(f"PRAGMA table_info({table})").fetchall()
    ]


def has_table(conn: sqlite3.Connection, name: str) -> bool:
    row = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
        (name,),
    ).fetchone()
    return row is not None


def resolve_targets(available: set[str]) -> list[str]:
    targets = ["duration_sec"]
    if "exit_speed_kmh" in available:
        targets.append("exit_speed_kmh")
    elif "end_speed_kmh" in available:
        targets.append("exit_speed_kmh")  # store/read under DB name
    return targets


def ensure_schema(conn: sqlite3.Connection) -> None:
    conn.executescript(TIMING_FACTORS_DDL)
    conn.commit()


def load_attempts(
    conn: sqlite3.Connection,
    track: str,
    car: str,
    direction: str,
    from_sector: int,
    to_sector: int,
    car_contains: bool,
) -> tuple[dict[LegKey, list[Attempt]], list[str]]:
    split_cols = set(table_columns(conn, "sector_splits"))
    metric_cols = [c for c in SPLIT_METRIC_COLUMNS if c in split_cols]

    # Legacy: optional sector_split_factors table (acr_telemetry experiment)
    join_factors = False
    factor_cols: list[str] = []
    if has_table(conn, "sector_split_factors"):
        factor_cols = [
            c
            for c in table_columns(conn, "sector_split_factors")
            if c not in ("split_id",) and c not in split_cols
        ]
        if factor_cols:
            join_factors = True
            if "end_speed_kmh" in factor_cols and "exit_speed_kmh" not in split_cols:
                metric_cols = list(metric_cols)

    select_parts = [
        "s.track_name",
        "s.car_model",
        "s.direction",
        "s.from_sector",
        "s.to_sector",
        "s.duration_sec",
    ]
    col_names = [
        "track_name",
        "car_model",
        "direction",
        "from_sector",
        "to_sector",
        "duration_sec",
    ]
    for c in metric_cols:
        select_parts.append(f"s.{c}")
        col_names.append(c)
    if join_factors:
        for c in factor_cols:
            select_parts.append(f"f.{c}")
            col_names.append(c)

    car_clause = "s.car_model LIKE ?" if car_contains else "s.car_model = ?"
    car_param = f"%{car}%" if car_contains else car

    from_clause = "sector_splits s"
    if join_factors:
        from_clause += " LEFT JOIN sector_split_factors f ON f.split_id = s.id"

    sql = f"""
SELECT {", ".join(select_parts)}
FROM {from_clause}
WHERE s.track_name = ?
  AND {car_clause}
  AND s.direction = ?
  AND s.from_sector >= ?
  AND s.to_sector <= ?
  AND s.to_sector > s.from_sector
ORDER BY s.from_sector, s.to_sector, s.created_at_utc
"""
    rows = conn.execute(
        sql,
        (track, car_param, direction, from_sector, to_sector),
    ).fetchall()

    available = set(metric_cols) | set(factor_cols)
    targets = resolve_targets(available)

    out: dict[LegKey, list[Attempt]] = {}
    for row in rows:
        row_map = dict(zip(col_names, row))
        key = LegKey(
            row_map["track_name"],
            row_map["car_model"],
            row_map["direction"],
            int(row_map["from_sector"]),
            int(row_map["to_sector"]),
        )
        duration = finite(row_map.get("duration_sec"))
        if duration is None:
            continue
        values: dict[str, float] = {"duration_sec": duration}
        for name in metric_cols + factor_cols:
            if name == "duration_sec":
                continue
            v = finite(row_map.get(name))
            if v is not None:
                values[name] = v
        # Alias legacy name → exit_speed_kmh for targets
        if "end_speed_kmh" in values and "exit_speed_kmh" not in values:
            values["exit_speed_kmh"] = values["end_speed_kmh"]
        out.setdefault(key, []).append(Attempt(duration_sec=duration, values=values))

    if "exit_speed_kmh" not in available and "end_speed_kmh" in available:
        targets = [
            "exit_speed_kmh" if t == "end_speed_kmh" else t for t in targets
        ]

    return out, targets


def filter_attempts(
    attempts: list[Attempt], slow_pct: float
) -> tuple[list[Attempt], float, float, int]:
    if not attempts:
        return [], float("nan"), float("nan"), 0
    best = min(a.duration_sec for a in attempts)
    limit = best * (1.0 + slow_pct / 100.0)
    kept = [a for a in attempts if a.duration_sec <= limit + 1e-9]
    return kept, best, limit, len(attempts)


def factors_for_leg(attempts: Sequence[Attempt], targets: Sequence[str]) -> list[str]:
    target_set = set(targets)
    names: set[str] = set()
    for a in attempts:
        names.update(a.values.keys())
    names -= target_set
    names.discard("duration_sec")
    return sorted(names)


def correlate_leg(
    key: LegKey,
    attempts: list[Attempt],
    targets: Sequence[str],
    slow_pct: float,
    min_samples: int,
) -> Iterable[dict[str, Any]]:
    filtered, best, limit, n_total = filter_attempts(attempts, slow_pct)
    if len(filtered) < min_samples:
        return []

    factors = factors_for_leg(filtered, targets)
    rows: list[dict[str, Any]] = []
    for target in targets:
        target_vals = [a.values.get(target) for a in filtered]
        if any(v is None for v in target_vals):
            continue
        y = [float(v) for v in target_vals]  # type: ignore[arg-type]
        for factor in factors:
            if factor == target:
                continue
            xs_raw = [a.values.get(factor) for a in filtered]
            if any(v is None for v in xs_raw):
                continue
            xs = [float(v) for v in xs_raw]  # type: ignore[arg-type]
            r = pearson(xs, y)
            rows.append(
                {
                    "track_name": key.track_name,
                    "car_model": key.car_model,
                    "direction": key.direction,
                    "from_sector": key.from_sector,
                    "to_sector": key.to_sector,
                    "target": target,
                    "factor": factor,
                    "correlation": r,
                    "n_samples": len(filtered),
                    "n_total": n_total,
                    "best_duration_sec": best,
                    "max_duration_sec": limit,
                }
            )
    return rows


def store_results(conn: sqlite3.Connection, rows: Sequence[dict[str, Any]]) -> int:
    if not rows:
        return 0
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%fZ")
    conn.executemany(
        """
INSERT INTO timing_factors (
    computed_at_utc, track_name, car_model, direction,
    from_sector, to_sector, target, factor,
    correlation, n_samples, n_total, best_duration_sec, max_duration_sec
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(track_name, car_model, direction, from_sector, to_sector, target, factor)
DO UPDATE SET
    computed_at_utc = excluded.computed_at_utc,
    correlation = excluded.correlation,
    n_samples = excluded.n_samples,
    n_total = excluded.n_total,
    best_duration_sec = excluded.best_duration_sec,
    max_duration_sec = excluded.max_duration_sec
""",
        [
            (
                now,
                r["track_name"],
                r["car_model"],
                r["direction"],
                r["from_sector"],
                r["to_sector"],
                r["target"],
                r["factor"],
                r["correlation"],
                r["n_samples"],
                r["n_total"],
                r["best_duration_sec"],
                r["max_duration_sec"],
            )
            for r in rows
        ],
    )
    conn.commit()
    return len(rows)


def print_summary(rows: Sequence[dict[str, Any]], targets: Sequence[str]) -> None:
    if not rows:
        print("No correlations written (not enough samples or missing metric columns).")
        return
    by_leg: dict[tuple[int, int], list[dict[str, Any]]] = {}
    for r in rows:
        by_leg.setdefault((r["from_sector"], r["to_sector"]), []).append(r)
    for (fs, ts), leg_rows in sorted(by_leg.items()):
        print(
            f"\nLeg {fs} -> {ts}  (n={leg_rows[0]['n_samples']}/{leg_rows[0]['n_total']}, "
            f"best={leg_rows[0]['best_duration_sec']:.3f}s, "
            f"cutoff={leg_rows[0]['max_duration_sec']:.3f}s)"
        )
        seen_targets = sorted({r["target"] for r in leg_rows}, key=lambda t: targets.index(t) if t in targets else 99)
        for target in seen_targets:
            target_rows = [r for r in leg_rows if r["target"] == target]
            print(f"  vs {target}:")
            ranked = sorted(
                target_rows,
                key=lambda r: abs(r["correlation"] or 0.0),
                reverse=True,
            )
            for r in ranked:
                rc = r["correlation"]
                rs = f"{rc:+.3f}" if rc is not None else "n/a"
                print(f"    {r['factor']:22s}  r={rs}")


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument(
        "--db",
        type=Path,
        default=STAGE_ROOT / "timing" / "timing.db",
        help="Path to timing SQLite database",
    )
    p.add_argument("--track", required=True)
    p.add_argument("--car", required=True)
    p.add_argument("--car-contains", action="store_true")
    p.add_argument("--direction", default="stage")
    p.add_argument("--from-sector", type=int, default=0)
    p.add_argument("--to-sector", type=int, default=99)
    p.add_argument("--slow-pct", type=float, default=10.0)
    p.add_argument("--min-samples", type=int, default=4)
    p.add_argument("--dry-run", action="store_true")
    return p.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    db_path = args.db.expanduser().resolve()
    if not db_path.is_file():
        print(f"Database not found: {db_path}", file=sys.stderr)
        return 1

    conn = sqlite3.connect(db_path)
    ensure_schema(conn)

    if not has_table(conn, "sector_splits"):
        print(f"No sector_splits table in {db_path}", file=sys.stderr)
        return 1

    legs, targets = load_attempts(
        conn,
        args.track,
        args.car,
        args.direction,
        args.from_sector,
        args.to_sector,
        args.car_contains,
    )
    if not legs:
        print(
            f"No splits for track={args.track!r} car={args.car!r} "
            f"direction={args.direction!r} sectors {args.from_sector}-{args.to_sector}",
            file=sys.stderr,
        )
        return 1

    split_cols = set(table_columns(conn, "sector_splits"))
    if "exit_speed_kmh" not in split_cols and "end_speed_kmh" not in split_cols:
        print(
            "Warning: sector_splits has no exit_speed_kmh column — "
            "only duration correlations possible.",
            file=sys.stderr,
        )

    all_rows: list[dict[str, Any]] = []
    for key, attempts in sorted(legs.items(), key=lambda kv: (kv[0].from_sector, kv[0].to_sector)):
        all_rows.extend(
            correlate_leg(key, attempts, targets, args.slow_pct, args.min_samples)
        )

    print_summary(all_rows, targets)

    if args.dry_run:
        print(f"\n(dry-run: {len(all_rows)} correlation rows not written)")
        return 0

    n = store_results(conn, all_rows)
    print(f"\nWrote {n} rows to timing_factors in {db_path}")
    if not any(r["target"] == "exit_speed_kmh" for r in all_rows):
        print(
            "Note: no exit_speed_kmh correlations — column NULL for all filtered attempts "
            "(re-drive with leg stats recording, or lower --min-samples).",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
