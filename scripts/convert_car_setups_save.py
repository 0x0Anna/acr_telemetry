#!/usr/bin/env python3
from __future__ import annotations

import argparse
from pathlib import Path

MARKER = b"CarSetupsSaveGameData\x00"
ENTRY_SEPARATOR = bytes.fromhex("0002000000ff0100000002000000")


def payload_size_offset(data: bytes) -> int:
    marker_pos = data.find(MARKER)
    if marker_pos < 0:
        raise ValueError("Marker 'CarSetupsSaveGameData' not found.")
    return marker_pos + len(MARKER)


def build_converted(new_template: bytes, old_source: bytes) -> bytes:
    new_size_off = payload_size_offset(new_template)
    old_size_off = payload_size_offset(old_source)

    # Keep all bytes before the payload size field from the new file.
    # Copy payload size + payload-to-EOF from the old file.
    return new_template[:new_size_off] + old_source[old_size_off:]


def build_converted_all_entries(new_template: bytes, old_source: bytes) -> bytes:
    new_size_off = payload_size_offset(new_template)
    old_size_off = payload_size_offset(old_source)

    old_size = int.from_bytes(old_source[old_size_off : old_size_off + 4], "little")
    old_following = old_source[old_size_off + 4 : old_size_off + 4 + old_size]
    if len(old_following) != old_size:
        raise ValueError("Old payload length is inconsistent.")
    if old_size < 8:
        raise ValueError("Old payload too short.")

    # Keep all old entries, but normalize the leading flag to the "new" style.
    # old: [u32 flag=0][u32 count=...][entries...]
    # new: [u32 flag=1][u32 count=...][entries...]
    normalized_following = (1).to_bytes(4, "little") + old_following[4:]
    payload = old_size.to_bytes(4, "little") + normalized_following
    return new_template[:new_size_off] + payload


def build_converted_single_first(new_template: bytes, old_source: bytes) -> bytes:
    new_size_off = payload_size_offset(new_template)
    old_size_off = payload_size_offset(old_source)

    old_size = int.from_bytes(old_source[old_size_off : old_size_off + 4], "little")
    old_following = old_source[old_size_off + 4 : old_size_off + 4 + old_size]
    if len(old_following) != old_size:
        raise ValueError("Old payload length is inconsistent.")

    sep = old_following.find(ENTRY_SEPARATOR)
    if sep < 0:
        raise ValueError("Could not detect first-entry separator in old payload.")
    if sep <= 8:
        raise ValueError("Detected separator too early; payload format unexpected.")

    # Keep one setup entry:
    # - force the two leading counters/flags to 1
    # - copy only bytes up to the first known entry separator
    single_following = (
        (1).to_bytes(4, "little")
        + (1).to_bytes(4, "little")
        + old_following[8:sep]
    )

    # In the first setup record, the u32 directly after the first car-name string
    # appears to be the number of filled slots for that car (5 in old multi saves).
    # Force it to 1 for a true single-entry payload.
    if len(single_following) >= 16:
        first_name_len = int.from_bytes(single_following[8:12], "little")
        count_off = 12 + first_name_len
        if 0 <= count_off <= len(single_following) - 4:
            single_following = (
                single_following[:count_off]
                + (1).to_bytes(4, "little")
                + single_following[count_off + 4 :]
            )
    single_size = len(single_following)
    single_payload = single_size.to_bytes(4, "little") + single_following
    return new_template[:new_size_off] + single_payload


def build_converted_first_car_five(new_template: bytes, old_source: bytes) -> bytes:
    new_size_off = payload_size_offset(new_template)
    old_size_off = payload_size_offset(old_source)

    old_size = int.from_bytes(old_source[old_size_off : old_size_off + 4], "little")
    old_following = old_source[old_size_off + 4 : old_size_off + 4 + old_size]
    if len(old_following) != old_size:
        raise ValueError("Old payload length is inconsistent.")

    sep_positions: list[int] = []
    start = 0
    while True:
        idx = old_following.find(ENTRY_SEPARATOR, start)
        if idx < 0:
            break
        sep_positions.append(idx)
        start = idx + 1

    if len(sep_positions) < 5:
        raise ValueError("Not enough setup separators found for five slots.")

    # Keep exactly the first five setup blocks for the first car.
    # Separator #5 starts block #6 (next car), so truncate there.
    end = sep_positions[4]
    first_car_five = (
        (1).to_bytes(4, "little")
        + (1).to_bytes(4, "little")
        + old_following[8:end]
    )

    # Force "filled slots for this car" to 5.
    first_name_len = int.from_bytes(first_car_five[8:12], "little")
    count_off = 12 + first_name_len
    if 0 <= count_off <= len(first_car_five) - 4:
        first_car_five = (
            first_car_five[:count_off]
            + (5).to_bytes(4, "little")
            + first_car_five[count_off + 4 :]
        )

    payload = len(first_car_five).to_bytes(4, "little") + first_car_five
    return new_template[:new_size_off] + payload


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description=(
            "Transfer CarSetups payload from an old multi-setup save into a new "
            "single-setup save container (GVAS header from new file is preserved)."
        )
    )
    p.add_argument("--old", required=True, help="Old save file (source payload).")
    p.add_argument("--new", required=True, help="New save file (destination template).")
    p.add_argument(
        "--out",
        required=True,
        help="Output save file path.",
    )
    p.add_argument(
        "--in-place",
        action="store_true",
        help="Also overwrite --new with converted data after writing --out.",
    )
    p.add_argument(
        "--single-first",
        action="store_true",
        help=(
            "Extract only the first setup entry from --old and write as a "
            "single-entry payload into the new container."
        ),
    )
    p.add_argument(
        "--first-car-five",
        action="store_true",
        help=(
            "Extract the first car with all five setup slots from --old and "
            "write as a single-car payload into the new container."
        ),
    )
    p.add_argument(
        "--all-entries",
        action="store_true",
        help=(
            "Transfer all entries from --old into the new container, while "
            "normalizing the leading payload flag to new-style format."
        ),
    )
    return p.parse_args()


def main() -> int:
    args = parse_args()
    old_path = Path(args.old)
    new_path = Path(args.new)
    out_path = Path(args.out)

    old_data = old_path.read_bytes()
    new_data = new_path.read_bytes()
    mode_count = sum([bool(args.single_first), bool(args.first_car_five), bool(args.all_entries)])
    if mode_count > 1:
        raise ValueError("Use only one mode flag: --single-first / --first-car-five / --all-entries.")

    if args.single_first:
        converted = build_converted_single_first(new_data, old_data)
    elif args.first_car_five:
        converted = build_converted_first_car_five(new_data, old_data)
    elif args.all_entries:
        converted = build_converted_all_entries(new_data, old_data)
    else:
        converted = build_converted(new_data, old_data)

    out_path.write_bytes(converted)

    if args.in_place:
        new_path.write_bytes(converted)

    new_size_off = payload_size_offset(new_data)
    old_size_off = payload_size_offset(old_data)
    old_payload_size = int.from_bytes(old_data[old_size_off : old_size_off + 4], "little")
    new_payload_size = int.from_bytes(new_data[new_size_off : new_size_off + 4], "little")
    out_payload_size = int.from_bytes(converted[new_size_off : new_size_off + 4], "little")

    print(f"old: {old_path} ({len(old_data)} bytes), payload={old_payload_size}")
    print(f"new: {new_path} ({len(new_data)} bytes), payload={new_payload_size}")
    print(f"out: {out_path} ({len(converted)} bytes), payload={out_payload_size}")
    print("Done.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
