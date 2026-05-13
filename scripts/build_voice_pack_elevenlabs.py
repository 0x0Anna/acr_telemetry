#!/usr/bin/env python3
"""Batch-generate PacenotePal-compatible WAV clips with the ElevenLabs API."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
import wave
from pathlib import Path
from typing import Any
from urllib import error, request

import yaml

PAUSE_RE = re.compile(r"^Pause(?P<sec>[\d.]+)s(?:_Reset)?$", re.IGNORECASE)
DIST_RE = re.compile(r"^Dist(?P<m>\d+)$", re.IGNORECASE)
TURN_NUM_RE = re.compile(r"^(Left|Right)(?P<n>[1-6])$", re.IGNORECASE)
TURN_STYLE_RE = re.compile(
    r"^(Left|Right)(?P<style>HP|OpenHP|AcuteHP|Flat|Square|Kink|ChicaneEntry|Chicane)$",
    re.IGNORECASE,
)
NUMBER_WORDS = {
    1: "one",
    2: "two",
    3: "three",
    4: "four",
    5: "five",
    6: "six",
    7: "seven",
    8: "eight",nd am, 
    9: "nine",
    10: "ten",
}

DEFAULT_MODEL_ID = "eleven_multilingual_v2"
DEFAULT_OUTPUT_FORMAT = "mp3_44100_128"
TARGET_SAMPLE_RATE = 44100
TARGET_CHANNELS = 1
TARGET_SAMPLE_WIDTH = 2
WINDOWS_FFMPEG_CANDIDATES = (
    "ffmpeg.exe",
    r"C:\ffmpeg\bin\ffmpeg.exe",
    r"C:\Program Files\ffmpeg\bin\ffmpeg.exe",
    r"C:\Program Files\Shotcut\ffmpeg.exe",
    r"C:\Program Files\MediathekView\bin\ffmpeg.exe",
    r"C:\Program Files\streamCapture2\ffmpeg.exe",
)


def resolve_ffmpeg(explicit: str | None = None) -> str | None:
    candidates: list[str] = []
    if explicit:
        candidates.append(explicit)
    for env_name in ("FFMPEG_PATH", "FFMPEG"):
        value = os.environ.get(env_name)
        if value:
            candidates.append(value)

    found = shutil.which("ffmpeg")
    if found:
        candidates.append(found)
    if os.name == "nt":
        candidates.extend(WINDOWS_FFMPEG_CANDIDATES)

    seen: set[str] = set()
    for candidate in candidates:
        normalized = str(Path(candidate))
        if normalized in seen:
            continue
        seen.add(normalized)
        path = Path(candidate)
        if path.is_file():
            return str(path)
        resolved = shutil.which(candidate)
        if resolved:
            return resolved
    return None


def collect_tokens(pacenotes_dir: Path) -> list[str]:
    tokens: set[str] = set()
    for yaml_path in sorted(pacenotes_dir.glob("*.yml")):
        if yaml_path.stem == "_blank":
            continue
        data = yaml.safe_load(yaml_path.read_text(encoding="utf-8"))
        if not isinstance(data, list):
            continue
        for note in data:
            for token in note.get("notes", []):
                if isinstance(token, str) and token:
                    tokens.add(token)
    return sorted(tokens)


def load_dictionary(path: Path | None) -> dict[str, str]:
    if path is None:
        return {}
    data = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError(f"Dictionary must be a YAML mapping: {path}")
    return {str(key): str(value) for key, value in data.items()}


def default_phrase(token: str) -> str | None:
    pause = PAUSE_RE.match(token)
    if pause:
        return None

    dist = DIST_RE.match(token)
    if dist:
        return dist.group("m")

    turn_num = TURN_NUM_RE.match(token)
    if turn_num:
        side = turn_num.group(1).lower()
        number = NUMBER_WORDS.get(int(turn_num.group("n")), turn_num.group("n"))
        return f"{side} {number}"

    turn_style = TURN_STYLE_RE.match(token)
    if turn_style:
        side = turn_style.group(1).lower()
        style = turn_style.group("style").lower()
        style_words = {
            "hp": "hairpin",
            "openhp": "open hairpin",
            "acutehp": "acute hairpin",
            "flat": "flat",
            "square": "square",
            "kink": "kink",
            "chicaneentry": "chicane entry",
            "chicane": "chicane",
        }
        return f"{side} {style_words.get(style, style)}"

    broken = re.sub(r"([a-z])([A-Z])", r"\1 \2", token)
    broken = broken.replace("_", " ").strip()
    return broken.lower()


def resolve_phrase(token: str, dictionary: dict[str, str]) -> str | None:
    if token in dictionary:
        return dictionary[token]
    return default_phrase(token)


def write_silence_wav(path: Path, seconds: float) -> None:
    frames = max(1, int(TARGET_SAMPLE_RATE * seconds))
    with wave.open(str(path), "wb") as wf:
        wf.setnchannels(TARGET_CHANNELS)
        wf.setsampwidth(TARGET_SAMPLE_WIDTH)
        wf.setframerate(TARGET_SAMPLE_RATE)
        wf.writeframes(b"\x00\x00" * frames)


def ffmpeg_to_wav(source: Path, target: Path, ffmpeg: str | None = None) -> None:
    ffmpeg = resolve_ffmpeg(ffmpeg)
    if ffmpeg is None:
        raise RuntimeError(
            "ffmpeg is required to convert MP3 to PCM WAV. Add ffmpeg to PATH, set FFMPEG_PATH, "
            "or pass --ffmpeg-path."
        )
    subprocess.run(
        [
            ffmpeg,
            "-y",
            "-i",
            str(source),
            "-ac",
            str(TARGET_CHANNELS),
            "-ar",
            str(TARGET_SAMPLE_RATE),
            "-sample_fmt",
            "s16",
            str(target),
        ],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def normalize_wav(path: Path) -> None:
    with wave.open(str(path), "rb") as wf:
        channels = wf.getnchannels()
        sample_width = wf.getsampwidth()
        sample_rate = wf.getframerate()
        frames = wf.readframes(wf.getnframes())
    if channels == TARGET_CHANNELS and sample_width == TARGET_SAMPLE_WIDTH and sample_rate == TARGET_SAMPLE_RATE:
        return
    tmp = path.with_suffix(path.suffix + ".tmp.wav")
    with wave.open(str(tmp), "wb") as wf:
        wf.setnchannels(TARGET_CHANNELS)
        wf.setsampwidth(TARGET_SAMPLE_WIDTH)
        wf.setframerate(TARGET_SAMPLE_RATE)
        wf.writeframes(frames)
    tmp.replace(path)


def elevenlabs_request(
    api_key: str,
    voice_id: str,
    text: str,
    *,
    model_id: str,
    output_format: str,
    stability: float,
    similarity_boost: float,
    style: float,
    use_speaker_boost: bool,
) -> bytes:
    url = f"https://api.elevenlabs.io/v1/text-to-speech/{voice_id}?output_format={output_format}"
    payload = {
        "text": text,
        "model_id": model_id,
        "voice_settings": {
            "stability": stability,
            "similarity_boost": similarity_boost,
            "style": style,
            "use_speaker_boost": use_speaker_boost,
        },
    }
    req = request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "xi-api-key": api_key,
            "Content-Type": "application/json",
            "Accept": "audio/mpeg",
        },
        method="POST",
    )
    with request.urlopen(req, timeout=120) as resp:
        return resp.read()


def list_voices(api_key: str) -> list[dict[str, Any]]:
    req = request.Request(
        "https://api.elevenlabs.io/v1/voices",
        headers={"xi-api-key": api_key},
        method="GET",
    )
    with request.urlopen(req, timeout=60) as resp:
        data = json.loads(resp.read().decode("utf-8"))
    return data.get("voices", [])


def synthesize_token(
    token: str,
    phrase: str | None,
    output_dir: Path,
    *,
    api_key: str,
    voice_id: str,
    model_id: str,
    output_format: str,
    stability: float,
    similarity_boost: float,
    style: float,
    use_speaker_boost: bool,
    dry_run: bool,
    ffmpeg_path: str | None,
) -> dict[str, Any]:
    target = output_dir / f"{token}.wav"
    if phrase is None:
        seconds = float(PAUSE_RE.match(token).group("sec"))  # type: ignore[union-attr]
        if dry_run:
            return {"token": token, "status": "dry_run_pause", "seconds": seconds, "path": str(target)}
        write_silence_wav(target, seconds)
        return {"token": token, "status": "pause", "seconds": seconds, "path": str(target)}

    if dry_run:
        return {"token": token, "status": "dry_run", "phrase": phrase, "path": str(target)}

    raw_suffix = ".mp3" if output_format.startswith("mp3") else ".bin"
    raw_path = target.with_suffix(raw_suffix)
    audio = elevenlabs_request(
        api_key,
        voice_id,
        phrase,
        model_id=model_id,
        output_format=output_format,
        stability=stability,
        similarity_boost=similarity_boost,
        style=style,
        use_speaker_boost=use_speaker_boost,
    )
    raw_path.write_bytes(audio)
    if raw_suffix == ".mp3":
        ffmpeg_to_wav(raw_path, target, ffmpeg_path)
        raw_path.unlink(missing_ok=True)
    else:
        raw_path.replace(target)
    normalize_wav(target)
    duration_sec = wave.open(str(target), "rb").getnframes() / TARGET_SAMPLE_RATE
    return {"token": token, "status": "ok", "phrase": phrase, "path": str(target), "duration_sec": round(duration_sec, 3)}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pacenotes-dir", type=Path, default=Path("vendor/PacenotePal/pacenotes"))
    parser.add_argument("--dictionary", type=Path, help="YAML map of token -> spoken phrase")
    parser.add_argument("--output-dir", type=Path, required=True, help="PacenotePal voice folder target")
    parser.add_argument("--voice-id", default=os.environ.get("ELEVENLABS_VOICE_ID"))
    parser.add_argument("--api-key", default=os.environ.get("ELEVENLABS_API_KEY"))
    parser.add_argument("--model-id", default=os.environ.get("ELEVENLABS_MODEL_ID", DEFAULT_MODEL_ID))
    parser.add_argument("--output-format", default=os.environ.get("ELEVENLABS_OUTPUT_FORMAT", DEFAULT_OUTPUT_FORMAT))
    parser.add_argument("--stability", type=float, default=0.45)
    parser.add_argument("--similarity-boost", type=float, default=0.8)
    parser.add_argument("--style", type=float, default=0.15)
    parser.add_argument("--speaker-boost", action="store_true", default=True)
    parser.add_argument("--no-speaker-boost", action="store_false", dest="speaker_boost")
    parser.add_argument("--sleep-ms", type=int, default=250, help="Delay between API calls")
    parser.add_argument("--limit", type=int, default=0, help="Only synthesize the first N tokens")
    parser.add_argument("--skip-existing", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--list-voices", action="store_true")
    parser.add_argument(
        "--ffmpeg-path",
        default=os.environ.get("FFMPEG_PATH") or os.environ.get("FFMPEG"),
        help="Path to ffmpeg.exe when it is not on PATH (or set FFMPEG_PATH)",
    )
    args = parser.parse_args()

    if args.list_voices:
        if not args.api_key:
            raise SystemExit("Set ELEVENLABS_API_KEY or pass --api-key to list voices.")
        for voice in list_voices(args.api_key):
            labels = voice.get("labels") or {}
            print(
                f"{voice.get('voice_id')}\t{voice.get('name')}\t"
                f"{labels.get('language', '')}\t{labels.get('gender', '')}"
            )
        return

    if not args.dry_run and not args.api_key:
        raise SystemExit("Set ELEVENLABS_API_KEY or pass --api-key.")
    if not args.voice_id:
        raise SystemExit("Set ELEVENLABS_VOICE_ID or pass --voice-id.")

    ffmpeg_path = resolve_ffmpeg(args.ffmpeg_path)
    if not args.dry_run and not ffmpeg_path:
        raise SystemExit(
            "ffmpeg was not found. Add it to PATH, set FFMPEG_PATH, or pass --ffmpeg-path."
        )
    if ffmpeg_path and not args.dry_run:
        print(f"Using ffmpeg: {ffmpeg_path}")

    tokens = collect_tokens(args.pacenotes_dir)
    dictionary = load_dictionary(args.dictionary)
    if args.limit > 0:
        tokens = tokens[: args.limit]

    args.output_dir.mkdir(parents=True, exist_ok=True)
    manifest: list[dict[str, Any]] = []
    for token in tokens:
        target = args.output_dir / f"{token}.wav"
        if args.skip_existing and target.exists():
            manifest.append({"token": token, "status": "skipped", "path": str(target)})
            continue
        phrase = resolve_phrase(token, dictionary)
        entry = synthesize_token(
            token,
            phrase,
            args.output_dir,
            api_key=args.api_key or "",
            voice_id=args.voice_id,
            model_id=args.model_id,
            output_format=args.output_format,
            stability=args.stability,
            similarity_boost=args.similarity_boost,
            style=args.style,
            use_speaker_boost=args.speaker_boost,
            dry_run=args.dry_run,
            ffmpeg_path=ffmpeg_path,
        )
        manifest.append(entry)
        print(json.dumps(entry, ensure_ascii=False))
        if not args.dry_run and phrase is not None and args.sleep_ms > 0:
            time.sleep(args.sleep_ms / 1000.0)

    manifest_path = args.output_dir / "manifest.json"
    if not args.dry_run:
        manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"Wrote manifest to {manifest_path}")


if __name__ == "__main__":
    main()
