#!/usr/bin/env bash
# MVP acceptance run (docs/IMPLEMENTATION.md §1, rows A1–A11, and the common cases C1–C10):
# a release build, every req_a* test in release (the long #[ignore]d A1/A2/A3 runs included; the
# untimed A11 desktop rows in the dev profile),
# the common-case tests, the adversarial suite, `omr bench`, and the gated real-data runs when
# their variables are set. Writes docs/evidence/acceptance.json: one object per row,
# aggregates only.
#
# Usage: scripts/acceptance.sh
#
# Environment:
#   CARGO_TARGET_DIR   build folder (default: <repo>/target-app)
#   OMR_ACCEPT_WORK    logs and per-test JSON (default: a new temporary folder, kept)
#   OMR_ACCEPT_JSON    where to write the report (default: docs/evidence/acceptance.json)
#   Real data (restricted personal data: every output stays under OMR_ACCEPT_WORK or the
#   given scratch folders, and only counts, rates and timings reach the report):
#   OMR_IAU_PDF_DIR + OMR_IAU_REFERENCE  omr-app real_iau_e2e (OMR_E2E_OUT, OMR_E2E_REPORT
#                                        default below OMR_ACCEPT_WORK) and `omr bench` on the PDFs
#   OMR_IAU_DIR                          omr-recog real_iau and ownform_real_iau
#   OMR_ACCEPT_A11=1                     also run the shell-slint A11 tests (builds Slint;
#                                        OMR_ACCEPT_A11_TARGET_DIR selects its build folder)
#
# Timing rows (A2, A3, A4) run one test at a time; the machine's load average is recorded
# before and after, because a loaded machine lowers throughput (R10: a failed timing row says
# to rerun on a quiet machine before touching code).
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target-app}"
WORK="${OMR_ACCEPT_WORK:-$(mktemp -d -t omr-accept-XXXXXX)}"
REPORT="${OMR_ACCEPT_JSON:-$ROOT/docs/evidence/acceptance.json}"
mkdir -p "$WORK/out" "$WORK/logs"
rm -f "$WORK/out"/*.json "$WORK/runs.tsv"
export OMR_ACCEPT_OUT="$WORK/out"
RUNS="$WORK/runs.tsv"
: > "$RUNS"
START_LOAD=$(cut -d' ' -f1-3 /proc/loadavg)
START_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)
echo "acceptance: work folder $WORK, target $CARGO_TARGET_DIR, load $START_LOAD"

fail() {
    echo "acceptance: $1" >&2
    echo "acceptance: remedy: $2" >&2
    exit 1
}

# Restricted data must never land in the repository (PDPL): refuse work folders inside it.
if [ -n "${OMR_IAU_PDF_DIR:-}${OMR_IAU_DIR:-}" ]; then
    for d in "$WORK" "${OMR_E2E_OUT:-$WORK}" "$(dirname "${OMR_E2E_REPORT:-$WORK/x}")"; do
        case "$(cd "$d" 2>/dev/null && pwd -P || echo "$d")/" in
        "$(pwd -P)"/*)
            fail "$d is inside the repository, and the real-data runs write graded exams there" \
                "set OMR_ACCEPT_WORK, OMR_E2E_OUT and OMR_E2E_REPORT to a scratch folder outside $ROOT"
            ;;
        esac
    done
fi

echo "acceptance: release build of the omr binary"
cargo build --release -p omr-cli >"$WORK/logs/build.log" 2>&1 ||
    fail "release build failed (see $WORK/logs/build.log)" \
        "fix the compile error, then rerun scripts/acceptance.sh"

# run ROW PACKAGE[/FEATURES] TARGET THREADS FILTER...  (TARGET: --lib or a test name)
run() {
    local row=$1 pkg=${2%%/*} spec=$2 target=$3 threads=$4
    shift 4
    local sel=(--test "$target")
    [ "$target" = "--lib" ] && sel=(--lib)
    [ "$spec" != "$pkg" ] && sel+=(--features "${spec#*/}")
    local log="$WORK/logs/$row-$pkg-${target#--}.log"
    local load0
    load0=$(cut -d' ' -f1 /proc/loadavg)
    local t0=$SECONDS
    # shellcheck disable=SC2086 # PROFILE is one flag or none
    cargo test ${PROFILE---release} -p "$pkg" "${sel[@]}" -- --include-ignored \
        --test-threads="$threads" "$@" >"$log" 2>&1
    local rc=$?
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$row" "$pkg" "${target#--}" "$rc" "$log" \
        "$load0" "$(cut -d' ' -f1 /proc/loadavg)" "$((SECONDS - t0))" >>"$RUNS"
    if [ $rc -ne 0 ]; then
        echo "acceptance: $row $pkg ${target#--}: FAILED (log: $log)"
    else
        echo "acceptance: $row $pkg ${target#--}: ok ($(grep -c ' \.\.\. ok$' "$log") tests)"
    fi
}

N=$(nproc)
# A1 recognition accuracy (own-form 50k held-out standard slice, hard slice, legacy synthetic).
run A1 omr-recog ownform_synth "$N" req_a1_
run A1 omr-recog synthetic "$N" req_a1_
run A1 omr-recog fractional_dpi "$N" req_a1_
run A1 omr-recog ownform_gates "$N" req_a1_
run A1 omr-app synthetic_e2e "$N" req_a1_
# A2 throughput and A3 time to first result (`omr bench`, `omr grade`), one at a time.
run A2 omr-cli acceptance_perf 1 req_a2_
run A2 omr-app adversarial 1 req_a2_
run A3 omr-cli acceptance_perf 1 req_a3_
# A4 regrade latency, one at a time.
run A4 omr-scoring regrade 1 req_a4_
run A4 omr-app synthetic_e2e 1 req_a4_
# A5 determinism.
run A5 omr-recog synthetic "$N" req_a5_
run A5 omr-recog ownform_determinism "$N" req_a5_
run A5 omr-app synthetic_e2e "$N" req_a5_
run A5 omr-app acceptance "$N" req_a5_
# A6 identity.
run A6 omr-identity a6_identity "$N" req_a6_
run A6 omr-recog ownform_gates "$N" req_a6_
# A7 page code.
run A7 omr-forms --lib "$N" req_a7_
run A7 omr-forms properties "$N" req_a7_
run A7 omr-forms/render render "$N" req_a7_
run A7 omr-recog --lib "$N" req_a7_
run A7 omr-recog ownform_identity "$N" req_a7_
run A7 omr-app acceptance "$N" req_a7_
# A8 scoring oracles.
run A8 omr-scoring a8_oracles "$N" req_a8_
run A8 omr-report golden "$N" req_a8_
run A8 omr-app synthetic_e2e "$N" req_a1_a8
# A9 outcome oracles.
run A9 omr-outcomes a9_oracles "$N" req_a9_
run A9 omr-report golden "$N" req_a9_
# A10 exceptions: catalogue scan and UNREADABLE never EMPTY.
run A10 omr-model catalogue "$N" req_a10_
run A10 omr-recog synthetic "$N" req_a10_
run A10 omr-recog ownform_identity "$N" req_a10_
run A10 omr-recog ownform_gates "$N" req_a10_
run A10 omr-identity a6_identity "$N" req_a10_
run A10 omr-app synthetic_e2e "$N" req_a10_
run A10 omr-app acceptance "$N" req_a10_
# A11 desktop UI (Slint build), only on request. Not a timing row: it runs in the dev profile,
# sharing the Slint build of scripts/check.sh instead of a second, release one.
if [ "${OMR_ACCEPT_A11:-}" = 1 ]; then
    CARGO_TARGET_DIR="${OMR_ACCEPT_A11_TARGET_DIR:-$CARGO_TARGET_DIR}" PROFILE= \
        run A11 shell-slint req_a11 1 req_a11_
    CARGO_TARGET_DIR="${OMR_ACCEPT_A11_TARGET_DIR:-$CARGO_TARGET_DIR}" PROFILE= \
        run A11 shell-slint req_a11_real 1 req_a11_
fi
# Common cases C1–C10 (docs/EVIDENCE.md §3).
run C omr-app iau_capture "$N" c1_ c2_ c3_
run C omr-app append_batch "$N" c4_
run C omr-app instructor_files "$N" c5_ c6_
run C omr-app project_actions "$N" c5_ c7_ c8_
run C omr-app iau_sheets "$N" c9_ c10_
run C omr-cli real_world "$N"
# Adversarial verification: hostile inputs through the whole application (the 500-page run
# above is its A2 part).
run V omr-app adversarial "$N" --skip req_a2_

# Real data (gated; aggregates only).
if [ -n "${OMR_IAU_PDF_DIR:-}" ] && [ -n "${OMR_IAU_REFERENCE:-}" ]; then
    export OMR_E2E_OUT="${OMR_E2E_OUT:-$WORK/real-e2e}"
    export OMR_E2E_REPORT="${OMR_E2E_REPORT:-$WORK/real-e2e.json}"
    mkdir -p "$OMR_E2E_OUT" "$(dirname "$OMR_E2E_REPORT")"
    rm -f "$OMR_E2E_REPORT"
    run R omr-app real_iau_e2e 1 req_a1_real
    cp "$OMR_E2E_REPORT" "$OMR_ACCEPT_OUT/real-e2e.json" 2>/dev/null || true
    # Scoring with the instructors' keys and key sheets (EVIDENCE §4.1).
    if [ -n "${OMR_IAU_KEYS:-}" ]; then
        run R omr-app real_iau_scored 1
    fi
    # The parallel run with Remark (EVIDENCE §5): pass or fail only. Its report and counts are
    # Restricted (spec §3.5, Q-08): they go to the private work folder, never to the evidence;
    # OMR_REMARK_EXPECT (restricted, outside the repository) holds what the run must find.
    if [ -n "${OMR_IAU_KEYS:-}" ] && [ -n "${OMR_REMARK_DIR:-}" ]; then
        export OMR_REMARK_REPORT="${OMR_REMARK_REPORT:-$WORK/remark-report}"
        mkdir -p "$OMR_REMARK_REPORT"
        chmod 700 "$OMR_REMARK_REPORT"
        run R omr-app real_iau_remark 1
    fi
    echo "acceptance: omr bench on the real PDFs"
    load0=$(cut -d' ' -f1 /proc/loadavg)
    if "$CARGO_TARGET_DIR/release/omr" bench "$OMR_IAU_PDF_DIR" --template iau-ccsit-v1 \
        --questions 29 --forms A,B --threads 1,2,4 --first 30 --repeat 3 \
        --scratch "$WORK/real-bench-scratch" --out "$OMR_ACCEPT_OUT/real-bench.json" \
        >"$WORK/logs/R-real-bench.log" 2>&1; then
        rc=0
    else
        rc=$?
    fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' R omr bench "$rc" "$WORK/logs/R-real-bench.log" \
        "$load0" "$(cut -d' ' -f1 /proc/loadavg)" 0 >>"$RUNS"
    rm -rf "$WORK/real-bench-scratch"
fi
if [ -n "${OMR_IAU_DIR:-}" ]; then
    export OMR_EVAL_OUT="$WORK/real-recog.json"
    rm -f "$OMR_EVAL_OUT"
    run R omr-recog real_iau 1 req_a1_real
    unset OMR_EVAL_OUT
    cp "$WORK/real-recog.json" "$OMR_ACCEPT_OUT/real-recog.json" 2>/dev/null || true
    run R omr-recog ownform_real_iau 1
fi

END_LOAD=$(cut -d' ' -f1-3 /proc/loadavg)
mkdir -p "$(dirname "$REPORT")"
python3 -I "$ROOT/scripts/acceptance_report.py" --work "$WORK" --out "$REPORT" \
    --start "$START_UTC" --load-start "$START_LOAD" --load-end "$END_LOAD"
rc=$?
[ $rc -ne 0 ] && [ $rc -ne 3 ] &&
    fail "the report could not be written" "see the Python error above; the logs are in $WORK/logs"
if [ $rc -eq 3 ] || awk -F'\t' '$4 != 0 { bad = 1 } END { exit !bad }' "$RUNS"; then
    fail "at least one acceptance row failed (see tests_failed in $REPORT and $WORK/logs)" \
        "open the failing log; for a timing row (A2, A3, A4) rerun on a quiet machine first; a \
row with no matching test means a test was renamed: update the run table in this script"
fi
echo "acceptance: all rows passed; report $REPORT"
