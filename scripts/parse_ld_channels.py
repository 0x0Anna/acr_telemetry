#!/usr/bin/env python3
"""Parse MoTeC .ld channel headers (ldparser-compatible layout)."""
import struct
import sys

CHAN_SIZE = 124


def parse_ld(path: str) -> list[dict]:
    with open(path, "rb") as f:
        data = f.read()
    meta_ptr = struct.unpack_from("<I", data, 8)[0]
    n_chans = struct.unpack_from("<I", data, 0x60)[0]
    off = meta_ptr
    channels = []
    for _ in range(n_chans + 10):
        if off + CHAN_SIZE > len(data):
            break
        prev, nxt, dptr, ndata, counter, dtype_a, dtype, freq = struct.unpack_from(
            "<IIIIHHHH", data, off
        )
        shift, mul, scale, dec = struct.unpack_from("<hhhh", data, off + 24)
        name = data[off + 32 : off + 64].split(b"\x00")[0].decode("ascii", errors="replace").strip()
        unit = data[off + 72 : off + 84].split(b"\x00")[0].decode("ascii", errors="replace").strip()
        if not name:
            break
        channels.append(
            {
                "name": name,
                "unit": unit,
                "freq": freq,
                "samples": ndata,
                "dtype": dtype,
                "dec": dec,
            }
        )
        if nxt == 0:
            break
        off = nxt
    return channels


if __name__ == "__main__":
    path = sys.argv[1] if len(sys.argv) > 1 else r"motec\Sample.ld"
    chans = parse_ld(path)
    print(f"{path}: {len(chans)} channels")
    for c in chans:
        print(f"  {c['name']:32} {c['unit']:8} freq={c['freq']:5} n={c['samples']:6}")
