# Nemotron streaming live-transcript evaluation

Date: 2026-08-25

Status: evidence complete, defer product integration

## Decision

Keep Nemotron as a reproducible research path, but do not package it or make it
a Minutes live-transcript engine in this release.

The model is genuinely streaming and meets the controlled latency target on
both Apple Silicon and a native Windows runner. It does not yet beat the whole
product tradeoff:

- the extracted English model is about 663 MB and used about 1.2 GB peak memory
  on the tested Mac;
- the Windows run used about 788 MB peak memory and more than one CPU-second per
  second of audio on a two-core GitHub runner;
- the official Windows Sherpa 1.13.6 package contains ONNX Runtime 1.17.1 even
  though the Sherpa API in that package requests ONNX Runtime API 27. The test
  worked only after replacing the bundled runtime with checksummed ONNX Runtime
  1.27.1 and staging the dependent DLLs beside the executable;
- Minutes currently uses `sherpa-rs` 0.6.8 and sherpa-onnx 1.12.9, which predates
  support for this model;
- the tested English model made a proper-name error on the public meeting
  fixture and disagreed slightly more with an existing final transcript than
  Apple progressive speech did on the private sample;
- the cross-platform Whisper draft path already supplies the product contract
  without a second large model download or a runtime migration.

Apple progressive speech is the better accelerator to continue validating on
macOS because it uses the operating system's speech stack and avoids the model,
memory, installer, and runtime-upgrade costs. Whisper remains the common fallback
until another common streaming engine proves enough user benefit to justify its
distribution cost.

## Candidate

The evaluated candidate was the 560 ms int8 English export:

`sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25`

NVIDIA describes the source model as a 600M-parameter cache-aware
FastConformer-RNNT model with native punctuation and capitalization. It supports
80, 160, 560, and 1120 ms chunks and is governed by the NVIDIA Open Model
License Agreement. Sherpa publishes pre-exported int8 ONNX packages for those
four chunk sizes.

Primary references:

- [NVIDIA model card](https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b)
- [Sherpa Nemotron streaming documentation](https://k2-fsa.github.io/sherpa/onnx/nemo/nemotron-streaming.html)

The downloaded artifacts were pinned by checksum in the evaluation workflow:

| Artifact | SHA-256 |
| --- | --- |
| Sherpa 1.13.6 Windows x64 shared runtime | `071d6641efd737a1f60de48c9c4cd596f78d5b0980815e8ad3798c95785d2b26` |
| ONNX Runtime 1.27.1 Windows x64 | `2e00414a63fdef0914cd5a5ede6c707844878e0c08e1b6693842f0451b2df2a1` |
| Nemotron English 560 ms int8 model | `78e2b79fcf7271553a74402a76b771b09ea40117a39566a79f52235b23db6358` |

The model archive was 463,945,051 bytes compressed and its extracted model
directory was 647,228 KiB on the test Mac.

## Method

The harness feeds 16 kHz mono PCM at actual speaking speed, polls the recognizer
after each 120 ms input slice, records every changed hypothesis, and adds 800 ms
of silence before the final drain. A 500 ms silent pre-roll was necessary to
avoid dropping the first spoken word. Product telemetry output is content-free.
Transcript text was emitted only for local quality scoring.

The measured fields are:

- model initialization time;
- speech start to first non-empty draft;
- count and maximum gap of changed draft revisions;
- end-of-input completion lag;
- process CPU time and peak resident memory.

Private audio and transcript text stayed on the Mac. GitHub Actions received
only the repository's public fixture and content-free metrics.

## Results

### Apple Silicon Mac

| Measure | Result |
| --- | --- |
| First non-empty draft | about 1.07 s on public fixtures |
| Draft cadence | about every 0.5 to 0.6 s |
| Completion lag | about 0.38 to 0.90 s |
| Peak resident memory | about 1.2 GB |
| CPU | roughly one full core |
| Clean dictation | exact against the fixture reference |
| Public meeting fixture | transcribed `Matt` as `Mad` |

A private 32.8-second sample produced its first update after 1.545 seconds, 37
changed hypotheses, and 36 revisions. Its final text had 7.04 percent word
disagreement against the existing 71-word Minutes final. Apple progressive
speech measured 5.63 percent on that same local comparison. These disagreement
figures are directional, not a general accuracy benchmark.

### Windows

The successful native run was GitHub Actions run
[32899579370](https://github.com/silverstein/minutes/actions/runs/32899579370)
at evaluation commit `46849615f082fa8cb219a1b0457f1b88a1068df6`.

Runner hardware:

- AMD EPYC 7763;
- 2 cores, 4 logical processors;
- 17,178,693,632 bytes of physical memory.

| Measure | Result |
| --- | ---: |
| Audio duration | 10,625.8 ms |
| Initialization | 3,121.6 ms |
| First non-empty draft | 991.7 ms |
| Changed drafts | 19 |
| Revisions | 18 |
| Maximum update gap | 617.1 ms |
| Completion lag | 328.4 ms |
| Process CPU time | 13,578.1 ms |
| Peak resident memory | 788,336,640 bytes |

The run passed the controlled product bounds of a first draft within two seconds,
completion within 1.5 seconds, and more than one revisable update.

## Packaging finding

The Windows result required a runtime repair that would be inappropriate to
hide inside product packaging. The official Sherpa 1.13.6 shared archive loaded
its bundled ONNX Runtime 1.17.1, then requested API 27. Merely placing ONNX
Runtime 1.27.1 earlier in `PATH` did not work because Windows resolved the DLL
beside the Sherpa module. The reproducible harness therefore stages the Sherpa
DLLs beside the executable and replaces the stale `onnxruntime.dll` with the
checksummed 1.27.1 DLL.

Any future integration must upgrade Minutes' Sherpa dependency as one reviewed
runtime unit and verify its native packages on macOS, Windows, and Linux. Minutes
must not silently splice runtime DLLs from separate release archives in the
shipping app.

## Revisit threshold

Reconsider a common streaming engine when a candidate can demonstrate all of
the following together:

- a materially better current-speech experience than the shipped Whisper draft
  path on representative meetings, names, accents, microphones, and long speech;
- a model and peak-memory cost appropriate for an optional local component;
- one supported runtime package per platform without manual library surgery;
- native macOS, Windows, and Linux acceptance, including capture-isolation and
  final-transcript preservation;
- a reviewed license, download, update, removal, offline, and fallback story.

Those conditions are tracked in beads `minutes-3zm3.10` and
`minutes-3zm3.11`. This evaluation satisfies `minutes-3zm3.9` with a defer
decision; it does not authorize a product dependency upgrade or a default
change.
