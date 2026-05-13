#!/usr/bin/env python3
"""Turn PacenotePal token lists into structured atoms and runtime flags."""

from __future__ import annotations

import re
from typing import Any

PAUSE_RE = re.compile(r"^Pause(?P<sec>[\d.]+)s(?:_Reset)?$", re.IGNORECASE)
DIST_RE = re.compile(r"^Dist(?P<m>\d+)$", re.IGNORECASE)
TURN_NUM_RE = re.compile(r"^(Left|Right)(?P<n>[1-6])$", re.IGNORECASE)
TURN_STYLE_RE = re.compile(
    r"^(Left|Right)(?P<style>HP|OpenHP|AcuteHP|Flat|Square|Kink|ChicaneEntry|Chicane)$",
    re.IGNORECASE,
)
TIGHTEN_RE = re.compile(r"^Tighten(?P<n>[1-5])$", re.IGNORECASE)

CONNECTOR_TOKENS = {"and", "into"}
LENGTH_TOKENS = {
    "long": "long",
    "verylong": "very_long",
    "short": "short",
}
MODIFIER_TOKENS = {
    "dontcut": ("modifier", "dont_cut"),
    "cut": ("modifier", "cut"),
    "smallcut": ("modifier", "small_cut"),
    "tightens": ("modifier", "tightens"),
    "tightenslate": ("modifier", "tightens_late"),
    "opens": ("opening", "opens"),
    "openslate": ("opening", "opens_late"),
    "widens": ("opening", "widens"),
    "narrows": ("modifier", "narrows"),
    "late": ("modifier", "late"),
    "sudden": ("modifier", "sudden"),
    "keepin": ("modifier", "keep_in"),
    "keepout": ("modifier", "keep_out"),
    "keepleft": ("modifier", "keep_left"),
    "keepright": ("modifier", "keep_right"),
    "keepmiddle": ("modifier", "keep_middle"),
    "caution": ("hazard", "caution"),
    "brake": ("hazard", "brake"),
    "slowdown": ("hazard", "slow_down"),
    "badcamber": ("hazard", "bad_camber"),
    "handbrake": ("hazard", "handbrake"),
    "heavybrake": ("hazard", "heavy_brake"),
    "finish": ("control", "finish"),
    "stopatmarshals": ("control", "stop_at_marshals"),
    "gostraight": ("control", "go_straight"),
    "goright": ("control", "go_right"),
    "goleft": ("control", "go_left"),
}
FEATURE_TOKENS = {
    "overcrest": "over_crest",
    "overbridge": "over_bridge",
    "overbumps": "over_bumps",
    "overbump": "over_bump",
    "intodip": "into_dip",
    "crest": "crest",
    "kinksstartingleft": "kinks_starting_left",
    "kinksstartingright": "kinks_starting_right",
    "kinks": "kinks",
}

# One active value per group; later tokens in the same callout replace earlier ones.
FLAG_GROUPS: dict[str, set[str]] = {
    "keep": {"keep_in", "keep_out", "keep_left", "keep_right", "keep_middle"},
    "cut": {"dont_cut", "cut", "small_cut"},
    "opening": {"opens", "opens_late", "widens"},
    "go": {"go_left", "go_straight", "go_right"},
}


def _atom(kind: str, source: str, **fields: Any) -> dict[str, Any]:
    atom = {"kind": kind, "source": source}
    atom.update(fields)
    return atom


def atomize_token(token: str) -> dict[str, Any]:
    source = token
    key = token.casefold().replace("_", "")

    if key in CONNECTOR_TOKENS:
        return _atom("connector", source, name=key)

    if key in LENGTH_TOKENS:
        return _atom("length", source, name=LENGTH_TOKENS[key])

    if key in MODIFIER_TOKENS:
        kind, name = MODIFIER_TOKENS[key]
        return _atom(kind, source, name=name)

    if key in FEATURE_TOKENS:
        return _atom("feature", source, name=FEATURE_TOKENS[key])

    pause = PAUSE_RE.match(token)
    if pause:
        return _atom(
            "pause",
            source,
            seconds=float(pause.group("sec")),
            reset=token.lower().endswith("_reset"),
        )

    dist = DIST_RE.match(token)
    if dist:
        return _atom("distance", source, meters=int(dist.group("m")))

    turn_num = TURN_NUM_RE.match(token)
    if turn_num:
        side = turn_num.group(1).lower()
        return _atom(
            "turn",
            source,
            side=side,
            severity=int(turn_num.group("n")),
        )

    turn_style = TURN_STYLE_RE.match(token)
    if turn_style:
        side = turn_style.group(1).lower()
        style = turn_style.group("style").lower()
        return _atom("turn", source, side=side, style=style)

    tighten = TIGHTEN_RE.match(token)
    if tighten:
        return _atom("modifier", source, name="tighten", level=int(tighten.group("n")))

    return _atom("other", source, name=token)


def _empty_flag_groups() -> dict[str, str | None]:
    return {group: None for group in FLAG_GROUPS}


def _assign_flag_group(groups: dict[str, str | None], group: str, value: str) -> None:
    groups[group] = value


def atomize_tokens(tokens: list[str]) -> dict[str, Any]:
    atoms = [atomize_token(token) for token in tokens]
    groups = _empty_flag_groups()
    bools: dict[str, bool] = {
        "caution": False,
        "brake": False,
        "finish": False,
        "narrows": False,
        "tightens": False,
        "tightens_late": False,
        "late": False,
        "sudden": False,
    }
    turn_sides: set[str] = set()
    turn_severities: list[int] = []
    distance_calls_m: list[int] = []
    pause_seconds: list[float] = []

    for atom in atoms:
        kind = atom["kind"]
        name = atom.get("name")
        if kind == "modifier" and name in FLAG_GROUPS["keep"]:
            _assign_flag_group(groups, "keep", name)
        elif kind == "modifier" and name in FLAG_GROUPS["cut"]:
            _assign_flag_group(groups, "cut", name)
        elif kind == "opening" and name in FLAG_GROUPS["opening"]:
            _assign_flag_group(groups, "opening", name)
        elif kind == "control" and name in FLAG_GROUPS["go"]:
            _assign_flag_group(groups, "go", name)
        elif kind == "modifier" and name == "narrows":
            bools["narrows"] = True
        elif kind == "modifier" and name == "tightens":
            bools["tightens"] = True
        elif kind == "modifier" and name == "tightens_late":
            bools["tightens_late"] = True
        elif kind == "modifier" and name == "late":
            bools["late"] = True
        elif kind == "modifier" and name == "sudden":
            bools["sudden"] = True
        elif kind == "hazard" and name == "caution":
            bools["caution"] = True
        elif kind == "hazard" and name in {"brake", "heavy_brake", "handbrake"}:
            bools["brake"] = True
        elif kind == "control" and name == "finish":
            bools["finish"] = True
        elif kind == "turn":
            turn_sides.add(atom["side"])
            if "severity" in atom:
                turn_severities.append(atom["severity"])
        elif kind == "distance":
            distance_calls_m.append(atom["meters"])
        elif kind == "pause":
            pause_seconds.append(atom["seconds"])

    turn: dict[str, Any] | None = None
    if turn_sides or turn_severities:
        turn = {}
        if turn_sides:
            turn["sides"] = sorted(turn_sides)
        if turn_severities:
            turn["severity_max"] = max(turn_severities)
            turn["severity_min"] = min(turn_severities)

    flags: dict[str, Any] = {
        "groups": groups,
        "bools": bools,
        "linked_to_next": False,
    }
    if turn is not None:
        flags["turn"] = turn
    if distance_calls_m:
        flags["distance_calls_m"] = distance_calls_m
    if pause_seconds:
        flags["pause_seconds"] = pause_seconds

    return {
        "tokens": list(tokens),
        "atoms": atoms,
        "flags": flags,
    }


GIS_GROUP_FIELDS = {f"grp_{group}": sorted(values) for group, values in FLAG_GROUPS.items()}
GIS_BOOL_FIELDS = [
    "caution",
    "brake",
    "finish",
    "narrows",
    "tightens",
    "tightens_late",
    "late",
    "sudden",
    "linked_to_next",
    "turn_left",
    "turn_right",
]


def gis_field_schema() -> dict[str, Any]:
    return {
        "group_fields": GIS_GROUP_FIELDS,
        "bool_fields": [f"flg_{name}" for name in GIS_BOOL_FIELDS],
        "notes": {
            "grp_*": "Mutually exclusive categorical flags for QGIS/WebGIS dropdown filters.",
            "flg_*": "Independent boolean flags for checkbox filters and rule-based styling.",
            "turn_sides": "Comma-separated turn sides when a numbered/style turn is present.",
            "notes_text": "Original PacenotePal token sequence for map labels.",
        },
    }


def gis_properties(flags: dict[str, Any], notes_text: str = "") -> dict[str, Any]:
    out: dict[str, Any] = {}
    for group, value in flags.get("groups", {}).items():
        out[f"grp_{group}"] = value or ""
    for name, value in flags.get("bools", {}).items():
        out[f"flg_{name}"] = bool(value)
    out["flg_linked_to_next"] = bool(flags.get("linked_to_next", False))

    turn = flags.get("turn")
    sides = turn.get("sides", []) if isinstance(turn, dict) else []
    out["turn_sides"] = ",".join(sides)
    out["flg_turn_left"] = "left" in sides
    out["flg_turn_right"] = "right" in sides
    if isinstance(turn, dict):
        if "severity_min" in turn:
            out["turn_severity_min"] = turn["severity_min"]
        if "severity_max" in turn:
            out["turn_severity_max"] = turn["severity_max"]

    distance_calls_m = flags.get("distance_calls_m")
    if distance_calls_m:
        out["distance_calls_m"] = ",".join(str(v) for v in distance_calls_m)
    pause_seconds = flags.get("pause_seconds")
    if pause_seconds:
        out["pause_seconds"] = ",".join(str(v) for v in pause_seconds)
    if notes_text:
        out["notes_text"] = notes_text
    return out
