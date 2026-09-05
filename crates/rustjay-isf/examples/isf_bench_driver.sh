#!/bin/bash
# Batch driver for the isf_bench example.
#
# Usage: isf_bench_driver.sh <shader-list.txt> <out.jsonl>
#
# Reads shader paths (one per line, blanks and #-comments skipped), runs each
# under `cargo run --release -p rustjay-isf --example isf_bench` with a 15s
# timeout, and appends one JSON line per shader to the output file:
#   {"path": "...", "ms": N.N}
#   {"path": "...", "error": "timeout|crash|compile"}
set -u

TIMEOUT_SECS=15

if [ $# -ne 2 ]; then
    echo "usage: $0 <shader-list.txt> <out.jsonl>" >&2
    exit 2
fi
LIST="$1"
OUT="$2"

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

cargo build --release -p rustjay-isf --example isf_bench || {
    echo "build failed" >&2
    exit 2
}
BIN="$ROOT/target/release/examples/isf_bench"

# macOS ships no `timeout`; prefer it or GNU coreutils' gtimeout if present,
# otherwise fall back to background process + sleep + kill.
HAVE_TIMEOUT=""
if command -v timeout >/dev/null 2>&1; then
    HAVE_TIMEOUT=timeout
elif command -v gtimeout >/dev/null 2>&1; then
    HAVE_TIMEOUT=gtimeout
fi

KILLED_FLAG=""
run_bench() {
    # stdout = bench JSON, rc: 0 ok, 124 timeout, 1 shader error, other crash
    if [ -n "$HAVE_TIMEOUT" ]; then
        "$HAVE_TIMEOUT" "$TIMEOUT_SECS" "$BIN" "$1"
        return $?
    fi
    KILLED_FLAG="$(mktemp -t isf_bench)"
    "$BIN" "$1" &
    local pid=$!
    ( sleep "$TIMEOUT_SECS" && kill -9 "$pid" 2>/dev/null && touch "$KILLED_FLAG.killed" ) &
    local watcher=$!
    wait "$pid" 2>/dev/null
    local rc=$?
    kill "$watcher" 2>/dev/null
    wait "$watcher" 2>/dev/null
    if [ -f "$KILLED_FLAG.killed" ]; then
        rm -f "$KILLED_FLAG" "$KILLED_FLAG.killed"
        return 124
    fi
    rm -f "$KILLED_FLAG"
    return $rc
}

json_escape() {
    # minimal escaping for paths: backslash and double-quote
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

: >"$OUT"
while IFS= read -r shader || [ -n "$shader" ]; do
    case "$shader" in '' | \#*) continue ;; esac
    errtmp="$(mktemp -t isf_bench_err)"
    out="$(run_bench "$shader" 2>"$errtmp")"
    rc=$?
    esc="$(json_escape "$shader")"
    ms="$(printf '%s' "$out" | sed -n 's/^{"ms": \([0-9.]*\), "frames": [0-9]*}$/\1/p')"
    if [ "$rc" -eq 0 ] && [ -n "$ms" ]; then
        echo "{\"path\": \"$esc\", \"ms\": $ms}" >>"$OUT"
    elif [ "$rc" -eq 124 ]; then
        echo "{\"path\": \"$esc\", \"error\": \"timeout\"}" >>"$OUT"
    elif [ "$rc" -eq 1 ]; then
        echo "{\"path\": \"$esc\", \"error\": \"compile\"}" >>"$OUT"
        sed 's/^/    /' "$errtmp" >&2
    else
        echo "{\"path\": \"$esc\", \"error\": \"crash\"}" >>"$OUT"
        sed 's/^/    /' "$errtmp" >&2
    fi
    rm -f "$errtmp"
done <"$LIST"
