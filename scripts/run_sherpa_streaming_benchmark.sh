#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 || $# -gt 6 ]]; then
  echo "usage: $0 <sherpa-runtime-root> <model-root> <audio.wav> [chunk-ms] [num-threads] [--content-free]" >&2
  exit 2
fi

runtime_root="$1"
model_root="$2"
audio_path="$3"
chunk_ms="${4:-120}"
num_threads="${5:-2}"
content_flag="${6:-}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
compiler="${CXX:-c++}"
binary="${TMPDIR:-/tmp}/minutes-sherpa-streaming-benchmark"

"$compiler" -std=c++17 -O2 \
  -I"$runtime_root/include" \
  "$script_dir/benchmark_sherpa_streaming.cpp" \
  -L"$runtime_root/lib" -lsherpa-onnx-cxx-api -lsherpa-onnx-c-api \
  -Wl,-rpath,"$runtime_root/lib" \
  -o "$binary"

args=("$model_root" "$audio_path" "$chunk_ms" "$num_threads")
if [[ -n "$content_flag" ]]; then
  if [[ "$content_flag" != "--content-free" ]]; then
    echo "the only supported sixth argument is --content-free" >&2
    exit 2
  fi
  args+=("$content_flag")
fi

exec "$binary" "${args[@]}"
