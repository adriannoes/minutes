#!/usr/bin/env python3
"""Run the paced Sherpa benchmark and emit content-free quality metrics."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path


def normalized_words(text: str) -> list[str]:
    return re.findall(r"[a-z0-9]+(?:'[a-z0-9]+)?", text.casefold())


def edit_distance(reference: list[str], hypothesis: list[str]) -> int:
    previous = list(range(len(hypothesis) + 1))
    for row, reference_word in enumerate(reference, start=1):
        current = [row]
        for column, hypothesis_word in enumerate(hypothesis, start=1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[column] + 1,
                    previous[column - 1]
                    + (0 if reference_word == hypothesis_word else 1),
                )
            )
        previous = current
    return previous[-1]


def read_reference(path: Path) -> str:
    lines: list[str] = []
    with path.open(encoding="utf-8") as handle:
        for raw_line in handle:
            try:
                value = json.loads(raw_line)
            except json.JSONDecodeError:
                continue
            text = value.get("text") if isinstance(value, dict) else None
            if isinstance(text, str) and text.strip():
                lines.append(text.strip())
    return " ".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("runner", type=Path)
    parser.add_argument("runtime_root", type=Path)
    parser.add_argument("model_root", type=Path)
    parser.add_argument("audio", type=Path)
    parser.add_argument("reference_jsonl", type=Path)
    parser.add_argument("--chunk-ms", default="120")
    parser.add_argument("--threads", default="2")
    args = parser.parse_args()

    completed = subprocess.run(
        [
            str(args.runner),
            str(args.runtime_root),
            str(args.model_root),
            str(args.audio),
            args.chunk_ms,
            args.threads,
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        print("paced Sherpa benchmark failed; transcript output was suppressed", file=sys.stderr)
        return completed.returncode

    try:
        metrics = json.loads(completed.stdout.strip().splitlines()[-1])
        hypothesis = metrics.pop("finalText")
        metrics.pop("textAtAudioEnd", None)
    except (IndexError, KeyError, json.JSONDecodeError, TypeError):
        print("paced Sherpa benchmark returned invalid metrics", file=sys.stderr)
        return 7

    reference_words = normalized_words(read_reference(args.reference_jsonl))
    hypothesis_words = normalized_words(hypothesis)
    if not reference_words:
        print("reference transcript contained no words", file=sys.stderr)
        return 8
    edits = edit_distance(reference_words, hypothesis_words)
    metrics.update(
        {
            "referenceWords": len(reference_words),
            "hypothesisWords": len(hypothesis_words),
            "wordErrors": edits,
            "wordErrorRatePct": round(100.0 * edits / len(reference_words), 2),
        }
    )
    print(json.dumps(metrics, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
