#!/usr/bin/env python3
"""Generate fabricated speech for local import QA; never accepts source recordings."""

import argparse
import array
import json
from pathlib import Path
import random
import subprocess
import tempfile
import wave

UTTERANCES = [
    (
        "Samantha",
        "The blue lantern is beside the window. Please leave it there until tomorrow morning.",
    ),
    (
        "Daniel",
        "We need seven yellow pencils for the drawing class. I will bring the paper and the brushes.",
    ),
    (
        "Samantha",
        "The garden gate will open at nine. Everyone should meet beside the fountain before we begin.",
    ),
    (
        "Daniel",
        "Our purple bicycle has a broken bell. The repair shop will replace it on Thursday afternoon.",
    ),
    (
        "Samantha",
        "The blue lantern is beside the window. Please leave it there until tomorrow morning.",
    ),
    (
        "Daniel",
        "That concludes our practice conversation. Thank you for helping with this artificial audio recording.",
    ),
]


def write_wav(path, samples, channels):
    with wave.open(str(path), "wb") as output:
        output.setparams((channels, 2, 16000, 0, "NONE", "not compressed"))
        output.writeframes(samples.tobytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--repeat", type=int, default=1)
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    mono = array.array("h", [0] * 16000)
    schedule = []
    with tempfile.TemporaryDirectory(prefix="loofah-synthetic-voices-") as temp:
        for index, (voice, text) in enumerate(UTTERANCES):
            aiff = Path(temp) / f"{index}.aiff"
            wav = Path(temp) / f"{index}.wav"
            subprocess.run(
                ["say", "-v", voice, "-r", "150", "-o", str(aiff), text], check=True
            )
            subprocess.run(
                [
                    "afconvert",
                    "-f",
                    "WAVE",
                    "-d",
                    "LEI16@16000",
                    "-c",
                    "1",
                    str(aiff),
                    str(wav),
                ],
                check=True,
            )
            with wave.open(str(wav), "rb") as source:
                speech = array.array("h", source.readframes(source.getnframes()))
            schedule.append(
                {"voice": voice, "text": text, "start_seconds": len(mono) / 16000}
            )
            mono.extend(speech)
            mono.extend([0] * 16000)
    mono *= args.repeat
    rng = random.Random(20260923)
    stereo = array.array("h")
    for index, sample in enumerate(mono):
        delayed = mono[index - 8] if index >= 8 else 0
        stereo.extend(
            (
                int(sample * 0.8),
                max(-32768, min(32767, int(delayed * 0.58 + rng.uniform(-8, 8)))),
            )
        )
    write_wav(args.output / "synthetic-mono.wav", mono, 1)
    write_wav(args.output / "synthetic-room.wav", stereo, 2)
    subprocess.run(
        [
            "afconvert",
            "-f",
            "m4af",
            "-d",
            "aac@48000",
            "-b",
            "192000",
            str(args.output / "synthetic-room.wav"),
            str(args.output / "synthetic-iphone.m4a"),
        ],
        check=True,
    )
    (args.output / "manifest.json").write_text(
        json.dumps(
            {
                "sample_rate": 16000,
                "rate": 150,
                "seed": 20260923,
                "repeat": args.repeat,
                "duration_seconds": len(mono) / 16000,
                "utterances": schedule,
            },
            indent=2,
        )
        + "\n"
    )
    print(f"Generated {len(mono) / 16000:.1f}s of fabricated speech in {args.output}")


if __name__ == "__main__":
    main()
