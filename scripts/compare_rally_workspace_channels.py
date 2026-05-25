#!/usr/bin/env python3
"""Compare Rally Basic workspace channel IDs vs Sample.ld vs ACR motec_ld export."""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from parse_ld_channels import parse_ld  # noqa: E402

WORKBOOK = Path(
    r"C:\Users\chdem\Documents\MoTeC\i2\Workspaces\Rally Basic\Workbooks\Default.i2wkb"
)
SAMPLE_LD = ROOT / "motec" / "Sample.ld"

# Channels emitted by src/export/motec_ld.rs (current default export)
ACR_LD_CHANNELS = [
    "Time", "Speed", "RPM", "Throttle", "Brake", "steering", "Gear",
    "speed", "throttle", "brake", "engineRotation", "gear_ok",
    "vecLinearAccelerationCar.x", "vecLinearAccelerationCar.y", "G ForceTotal",
    "LF.suspensionTravel", "RF.suspensionTravel", "LB.suspensionTravel", "RB.suspensionTravel",
    "LF.deflection", "RF.deflection", "LB.deflection", "RB.deflection",
    "car.pos.x", "car.pos.y", "car.pos.z",
    "LF.wheelPos.x", "LF.wheelPos.y", "LF.wheelPos.z",
    "RF.wheelPos.x", "RF.wheelPos.y", "RF.wheelPos.z",
    "LB.wheelPos.x", "LB.wheelPos.y", "LB.wheelPos.z",
    "RB.wheelPos.x", "RB.wheelPos.y", "RB.wheelPos.z",
    "LF.tyreTemperature", "RF.tyreTemperature", "LB.tyreTemperature", "RB.tyreTemperature",
    "LF.pressure", "RF.pressure", "LB.pressure", "RB.pressure",
    "LF.brakeDiskTempC", "RF.brakeDiskTempC", "LB.brakeDiskTempC", "RB.brakeDiskTempC",
    "LF.tyreWear%", "RF.tyreWear%", "LB.tyreWear%", "RB.tyreWear%",
    "position.x", "position.y", "position.z",
]

# Proposed Rally profile mapping (workspace Id -> LD channel name + unit hint)
RALLY_MAP = {
    "Engine RPM": ("Engine RPM", "rpm", "rpm"),
    "Throttle Pos": ("Throttle Pos", "%", "gas * 100"),
    "Steered Angle": ("Steered Angle", "deg", "steer_angle"),
    "G Force Lat": ("G Force Lat", "G", "g_force.x"),
    "G Force Long": ("G Force Long", "G", "g_force.y"),
    "Gear": ("Gear", "", "gear"),
    "Brake Status": ("Brake Status", "", "brake > threshold"),
    "Corr Speed": ("Ground Speed", "km/h", "speed_kmh (i2 RallyMaths may refine)"),
    "Ground Speed": ("Ground Speed", "km/h", "speed_kmh"),
    "Wheel Speed FL": ("Wheel Speed FL", "km/h", "from wheel_angular_speed FL"),
    "Wheel Speed FR": ("Wheel Speed FR", "km/h", "from wheel_angular_speed FR"),
    "Wheel Speed RL": ("Wheel Speed RL", "km/h", "from wheel_angular_speed RL"),
    "Wheel Speed RR": ("Wheel Speed RR", "km/h", "from wheel_angular_speed RR"),
    "Wheel Speed Front": ("Wheel Speed Front", "km/h", "avg(FL, FR)"),
    "Wheel Speed Rear": ("Wheel Speed Rear", "km/h", "avg(RL, RR)"),
    "Wheel Slip": ("Wheel Slip", "", "wheel_slip aggregate"),
    "Damper Pos FL": ("Damper Pos FL", "mm", "suspension_travel FL * 1000"),
    "Damper Pos FR": ("Damper Pos FR", "mm", "suspension_travel FR * 1000"),
    "Damper Pos RL": ("Damper Pos RL", "mm", "suspension_travel RL * 1000"),
    "Damper Pos RR": ("Damper Pos RR", "mm", "suspension_travel RR * 1000"),
    # Engine sheet sim aliases (RBR-style, already in ACR)
    "speed": ("speed", "km/h", "speed_kmh"),
    "steering": ("steering", "", "steer_angle"),
    "brake": ("brake", "%", "brake * 100"),
    "car.pos.x": ("car.pos.x", "m", "wheel centroid x"),
    "vecLinearAccelerationCar.x": ("vecLinearAccelerationCar.x", "g", "g_force.x"),
    "vecLinearAccelerationCar.y": ("vecLinearAccelerationCar.y", "g", "g_force.y"),
    "LF.suspensionTravel": ("LF.suspensionTravel", "m", "suspension_travel FL"),
    "RF.suspensionTravel": ("RF.suspensionTravel", "m", "suspension_travel FR"),
}


def workspace_channel_ids(text: str) -> set[str]:
    ids = set()
    for pat in (
        r'Trace Id="([^"]+)"',
        r'GaugeData Id="([^"]+)"',
        r'HistoTrace Id="([^"]+)"',
        r'FFTTrace ChanId="([^"]+)"',
        r'XChannel="([^"]+)"',
        r'ZChannel="([^"]+)"',
        r'SecondChannel="([^"]+)"',
        r'ChannelEntry ID="([^"]+)"',
    ):
        ids.update(re.findall(pat, text))
    return ids


def main() -> None:
    wb = WORKBOOK.read_text(encoding="utf-8", errors="replace")
    ws_ids = workspace_channel_ids(wb)
    sample = {c["name"] for c in parse_ld(str(SAMPLE_LD))}
    acr = set(ACR_LD_CHANNELS)

    print("=== Rally Basic workspace: unique channel IDs referenced ===")
    print(f"Count: {len(ws_ids)}\n")

    driver_style = {
        "Engine RPM", "Corr Speed", "Throttle Pos", "Steered Angle",
        "G Force Lat", "G Force Long", "Gear", "Brake Status",
    }
    drivetrain = {
        "Ground Speed", "Wheel Speed Front", "Wheel Speed Rear",
        "Wheel Speed FL", "Wheel Speed FR", "Wheel Speed RL", "Wheel Speed RR",
        "Wheel Slip", "Centre Diff Lock", "SDC Mode",
    }
    suspension = {
        "Damper Pos FL", "Damper Pos FR", "Damper Pos RL", "Damper Pos RR",
        "Damper Vel FL", "Damper Vel FR", "Damper Vel RL", "Damper Vel RR",
    }
    engine_ecu = {
        "Engine Temp", "Air Temp Inlet", "Eng Oil Temp", "Eng Oil Pres",
        "Fuel Pres", "Lambda 1", "Lambda 2", "Manifold Pres", "Ign Advance",
        "Fuel Inj Duty", "Battery Volts", "Bat Volts ADL",
    }
    sim_aliases = {
        "speed", "steering", "brake", "car.pos.x",
        "vecLinearAccelerationCar.x", "vecLinearAccelerationCar.y",
        "LF.suspensionTravel", "RF.suspensionTravel",
    }

    def report(title: str, group: set[str]) -> None:
        g = group & ws_ids
        print(f"--- {title} ({len(g)}) ---")
        for name in sorted(g):
            in_sample = "yes" if name in sample else "no"
            in_acr = "yes" if name in acr else "no"
            mapped = RALLY_MAP.get(name)
            map_str = f" -> LD `{mapped[0]}` ({mapped[1]})" if mapped else ""
            print(f"  {name:28} sample.ld={in_sample:3}  acr_ld={in_acr:3}{map_str}")
        missing = g - sample - acr
        if missing:
            print(f"  (needs export or i2 maths: {', '.join(sorted(missing))})")
        print()

    report("Driver worksheet (classic ADL names)", driver_style)
    report("Drivetrain / wheels", drivetrain)
    report("Suspension", suspension)
    report("Engine ECU (logger only)", engine_ecu)
    report("Sim-style (Engine sheet)", sim_aliases)

    unclassified = ws_ids - driver_style - drivetrain - suspension - engine_ecu - sim_aliases
    if unclassified:
        print(f"--- Other workspace refs ({len(unclassified)}) ---")
        for name in sorted(unclassified):
            print(f"  {name}")
        print()

    print("=== Coverage summary ===")
    covered_by_sample = ws_ids & sample
    covered_by_acr = ws_ids & acr
    print(f"In Sample.ld: {len(covered_by_sample)}/{len(ws_ids)}")
    print(f"In current ACR LD: {len(covered_by_acr)}/{len(ws_ids)}")
    print(f"In either: {len(ws_ids & (sample | acr))}/{len(ws_ids)}")
    need_export = ws_ids - sample - acr
    print(f"\nWorkspace refs with no Sample.ld and no ACR channel ({len(need_export)}):")
    for name in sorted(need_export):
        hint = RALLY_MAP.get(name, ("?", "?", "no sim data"))[2]
        print(f"  {name:28}  ({hint})")


if __name__ == "__main__":
    main()
