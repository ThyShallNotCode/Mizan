#!/usr/bin/env python3
"""Builds docs/evidence/acceptance.json from an acceptance run (scripts/acceptance.sh).

Reads the run table (`runs.tsv`), the cargo test logs (test names and outcomes) and the JSON
aggregates the tests wrote to `<work>/out`. Writes one object per acceptance row: id,
requirement, method, measured, target, pass and the tests that prove it. Aggregates only:
counts, rates and timings; the real-data section never carries a file name, a student number
or a per-student value.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys

ROWS = [
    {
        "id": "A1",
        "requirement": "Recognition accuracy with calibrated abstention (spec §8, §23.1)",
        "method": "Held-out semi-synthetic standard slice from omr-sim (truth by construction): "
        "500 own-form sheets x 100 questions, 200-dpi gray JPEG q70-95, seed 20261016 (never used "
        "for tuning); pooled-batch and single-page baselines. Hard slice (light marks, erasures, "
        "ticks, cross-outs, strong distortion, streaks) on the same seed, reported separately. "
        "Plus the committed small corpora and the legacy synthetic tests.",
        "target": "0 silent errors on >= 50,000 test questions, SER bound 3/n at k = 0, AR <= 1 %",
    },
    {
        "id": "A2",
        "requirement": "Throughput (spec §2.4, §21.1)",
        "method": "omr bench (release) at 1, 2 and 4 threads, best of 3: decode + page stage + batch "
        "stage (recognition) and the full in-memory pipeline, on 240 own-form 200-dpi JPEG sheets "
        "and on 150 synthetic legacy IAU sheets as 600-dpi bitonal PDFs; plus one 500-page "
        "own-form PDF graded through the whole application (store included) at 4 threads, with "
        "its peak memory",
        "target": ">= 40 pages/s at 4 threads (spec 4-core floor); stretch >= 100",
    },
    {
        "id": "A3",
        "requirement": "Time to first result, 30 sheets (spec §2.4)",
        "method": "Wall time of the `omr grade` process on exactly 30 sheets with key, roster and "
        "CLO file at 4 threads (start, read, spool, recognition, identity, scoring, outcomes, "
        "analytics, store, every export), own forms and legacy; plus omr bench's in-process "
        "30-sheet run",
        "target": "< 3 s",
    },
    {
        "id": "A4",
        "requirement": "Regrade latency, 10,000 sheets x 100 items (spec §10.1)",
        "method": "Release, best of 5: omr-scoring key-change regrade (10 items re-keyed, 4 "
        "permuted forms, validate + compile + L3-L6) and the omr-app session rescoring after one "
        "review action and after a key change; reported beside it, a review action including the "
        "save of the session snapshot bound to the audit chain (CR-14)",
        "target": "<= 50 ms",
    },
    {
        "id": "A5",
        "requirement": "Determinism (spec §8.11, §23.6)",
        "method": "The same corpus graded at 1, 2 and 4 threads in name order, in a seeded "
        "shuffle and reversed, compared byte for byte (own forms: 24 sheets; legacy: 18 sheets "
        "in two slices); engine-level measurement identity at 1/2/4 rayon threads",
        "target": "Byte-identical decision files",
    },
    {
        "id": "A6",
        "requirement": "Identity (spec §9)",
        "method": "Oracle tests of roster matching from the bubbled ID, national ID/Iqama "
        "detection, duplicates, not-on-roster, rosterless mode, assignment invariants",
        "target": "Correct codes: IDN-004 sub-reasons, IDN-005",
    },
    {
        "id": "A7",
        "requirement": "Page code (spec §6.4)",
        "method": "Encode/decode round trip (unit + property), single-character changes, other "
        "keys, rendered sheets at 200/300 dpi, every copy outcome on rendered sheets, forged and "
        "foreign sheets through the whole app",
        "target": "96-bit HMAC-SHA384 tag verified; IDN-002 on failure",
    },
    {
        "id": "A8",
        "requirement": "Scoring (spec §10)",
        "method": "Hand-computed oracles: multi-form mapping, item rules, exact rationals, Saudi "
        "grade scale, voids/bonus/choice groups, floors, penalties; golden CSV; end-to-end "
        "scores equal a hand computation from the truth",
        "target": "Exact match with hand-computed oracles",
    },
    {
        "id": "A9",
        "requirement": "Outcomes (spec §11)",
        "method": "Hand-computed oracles: average, threshold proportion, Taif per-question, "
        "FCAR/EAMU, TP-154 A.1/B.1 rows, PLO roll-up, ABET crosswalk, missing data; golden CSV",
        "target": "Exact match with hand-computed oracles",
    },
    {
        "id": "A10",
        "requirement": "Exceptions (spec §13.1, Appendix A)",
        "method": "Source scan of every .rs/.slint file for exception codes against the generated "
        "Appendix A catalogue (plus literal severity/scope at constructor calls); targeted tests "
        "that cut-off, unregistered and two-tag pages are UNREADABLE (never blank) and stay "
        "pending through scoring",
        "target": "Every raised code exists in Appendix A; UNREADABLE is never EMPTY",
    },
    {
        "id": "A11",
        "requirement": "Desktop UI (spec §5.2, ADR-001 S7)",
        "method": "shell-slint tests (OMR_ACCEPT_A11=1, dev profile: not timed): headless "
        "screenshots of every screen in both themes and window sizes; review flow driven "
        "through accessible labels and keys; the real backend over omr-app (review, live "
        "grading, added batches, the New-exam form, key replacement, release). Screenshots: "
        "docs/design/screens/, docs/design/screens/real/",
        "target": "Screens render; scripted review completes",
    },
]

COMMON = {
    "C1": "Orientation: pages turned 90/180/270 degrees",
    "C2": "Duplex blank backs (CAP-012)",
    "C3": "Mixed capture slices in one exam",
    "C4": "A later batch appended",
    "C5": "Answer keys: real key after --infer-key, mis-key, file layouts",
    "C6": "Rosters: BOM, Arabic, Excel damage, duplicates",
    "C7": "Override reasons on review actions",
    "C8": "Blackboard grade-center upload",
    "C9": "Absent, partial, stray-mark and full-row sheets",
    "C10": "Student-ID mistakes on IAU sheets",
}

ADVERSARIAL = (
    "Hostile inputs through the whole application (omr-app/tests/adversarial.rs, synthetic "
    "data): broken, truncated, encrypted and mislabelled files, multi-page TIFF, foreign and "
    "unreadable pages, 75/150/300/400 dpi, rescans and duplicated pages, every bubble filled "
    "or none marked, void and any-of keys, review + key change + undo, append + undo, a group "
    "decision refused part way, determinism across threads and input orders"
)

TEST_LINE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)$")


def load(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def runs_of(work):
    runs = []
    with open(os.path.join(work, "runs.tsv"), encoding="utf-8") as f:
        for line in f:
            row, pkg, target, rc, log, load0, load1, secs = line.rstrip("\n").split("\t")
            tests = []
            if pkg == "omr" and target == "bench":
                # `omr bench` itself (not a cargo test): its exit status is the result.
                tests.append(("omr_bench_real_iau_pdfs", "ok" if rc == "0" else "FAILED"))
            try:
                with open(log, encoding="utf-8", errors="replace") as lf:
                    for l in lf:
                        m = TEST_LINE.match(l.strip())
                        if m:
                            tests.append((m.group(1), m.group(2)))
            except OSError:
                pass
            runs.append(
                {
                    "row": row,
                    "package": pkg,
                    "target": target,
                    "exit": int(rc),
                    "tests": tests,
                    "load_before": float(load0),
                    "load_after": float(load1),
                    "seconds": int(secs),
                }
            )
    return runs


def tidy(v):
    """Floats to 6 significant digits (timings and rates, not identifiers)."""
    if isinstance(v, float):
        return float(f"{v:.6g}")
    if isinstance(v, dict):
        return {k: tidy(x) for k, x in v.items()}
    if isinstance(v, list):
        return [tidy(x) for x in v]
    return v


def upper95(k, n):
    """One-sided 95 % upper bound for k events in n (§23.1 table; None beyond k = 2)."""
    c = {0: 3.00, 1: 4.74, 2: 6.30}.get(k)
    return None if c is None or not n else c / n


def pick(d, *keys):
    return {k: d.get(k) for k in keys if d is not None and k in d}


def decisions(v):
    if v is None:
        return None
    out = {"corpus": pick(v.get("corpus", {}), "profile", "seed", "sheets", "pages",
                          "questions_per_sheet", "options", "forms", "dpi")}
    out["registration"] = pick(v.get("registration", {}), "failed_pages", "code_not_verified",
                               "wrong_page_or_payload", "true_centre_error_px_max")
    for mode in ("pooled_batch", "single_page_prior"):
        d = v.get("decisions", {}).get(mode)
        if d is None:
            continue
        out[mode] = pick(d, "questions", "auto_accepted", "silent_errors", "ser_per_question",
                         "ser_per_question_upper95", "ser_per_question_upper95_deff2", "review",
                         "ar", "auto_accepted_on_review_truth", "sheets",
                         "sheets_with_silent_error", "sheet_ser_upper95", "student_id")
    out["timing_one_thread"] = v.get("timing")
    return out


def bench_summary(b):
    if b is None:
        return None
    runs = {}
    for r in b.get("bench", b).get("runs", []):
        runs[str(r["threads"])] = {
            "recognition_pages_per_s": round(r["recognition_pages_per_s"], 1),
            "pipeline_pages_per_s": round(r["pipeline_pages_per_s"], 1),
        }
    ttfr = b.get("bench", b).get("ttfr", {})
    return {
        "pages": b.get("corpus_pages", (b.get("runs") or [{}])[0].get("pages")),
        "pages_per_s_by_threads": runs,
        "bench_30_sheets_in_process_s": round(ttfr.get("seconds", 0.0), 3) if ttfr else None,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--start", required=True)
    ap.add_argument("--load-start", required=True)
    ap.add_argument("--load-end", required=True)
    a = ap.parse_args()
    out_dir = os.path.join(a.work, "out")
    j = lambda name: load(os.path.join(out_dir, name + ".json"))
    runs = runs_of(a.work)

    def tests_of(row):
        names, failed, ran = [], [], False
        for r in runs:
            if r["row"] != row:
                continue
            ran = True
            for name, status in r["tests"]:
                label = f"{r['package']}::{r['target']}::{name}"
                (names if status == "ok" else failed).append(label)
            if r["exit"] != 0 and not any(s != "ok" for _, s in r["tests"]):
                failed.append(f"{r['package']}::{r['target']} (exit {r['exit']}, no test result)")
            elif not r["tests"]:
                failed.append(f"{r['package']}::{r['target']} (no test matched the filter)")
        # Sorted: test threads finish in any order, and the report must not change with it.
        return ran, sorted(set(names)), sorted(set(failed))

    measured = {
        "A1": {
            "standard_slice": decisions(j("a1-ownform-standard")),
            "hard_slice_reported_separately": decisions(j("a1-ownform-hard")),
        },
        "A2": {
            "ownform_200dpi_jpeg": bench_summary(j("a2-ownform-200dpi-jpeg")),
            "legacy_iau_600dpi_bitonal_pdf": bench_summary(j("a2-legacy-600dpi-bitonal-pdf")),
            "large_pdf_500_pages_whole_app": j("a2-large-pdf-500"),
        },
        "A3": j("a3-time-to-first-result"),
        "A4": {
            "omr_scoring_key_change": pick(j("a4-scoring-regrade") or {}, "booklets", "items",
                                           "forms", "key_change_regrade_ms", "prepare_ms",
                                           "release_build"),
            "omr_app_session": pick(j("a4-app-rescoring") or {}, "booklets", "items",
                                    "review_action_rescore_ms", "key_change_rescore_ms",
                                    "review_action_with_snapshot_save_ms", "snapshot_bytes",
                                    "release_build"),
        },
        "A5": {"ownform": j("a5-ownform-determinism"), "legacy": j("a5-legacy-determinism")},
        "A10": {"catalogue_scan": j("a10-catalogue-scan")},
    }
    rows = []
    for meta in ROWS:
        ran, names, failed = tests_of(meta["id"])
        m = measured.get(meta["id"]) or {}
        timing = [r for r in runs if r["row"] == meta["id"] and meta["id"] in ("A2", "A3", "A4")]
        if timing:
            m = dict(m)
            m["load_average_during"] = [max(r["load_before"], r["load_after"]) for r in timing]
        m["tests_passed"] = len(names)
        row = dict(meta)
        row["measured"] = m
        row["pass"] = (not failed and len(names) > 0) if ran else None
        if not ran:
            row["note"] = ("not run by this acceptance run (set OMR_ACCEPT_A11=1); the GUI "
                           "agent runs these tests" if meta["id"] == "A11" else "not run")
        row["tests"] = names
        row["tests_failed"] = failed
        rows.append(row)

    ran_c, c_names, c_failed = tests_of("C")
    common = []
    for cid, what in COMMON.items():
        prefix = cid.lower() + "_"
        ts = [n for n in c_names if n.split("::")[-1].startswith(prefix)]
        fs = [n for n in c_failed if n.split("::")[-1].startswith(prefix)]
        if cid in ("C4", "C5", "C6", "C7", "C8"):
            ts += [n for n in c_names if "real_world" in n]
        common.append({"id": cid, "case": what, "tests": ts, "tests_failed": fs,
                       "pass": (bool(ts) and not fs) if ran_c else None})

    ran_v, v_names, v_failed = tests_of("V")
    adversarial = None
    if ran_v:
        adversarial = {"what": ADVERSARIAL, "tests": v_names, "tests_failed": v_failed,
                       "pass": not v_failed and len(v_names) > 0}

    real = None
    e2e, recog, bench = j("real-e2e"), j("real-recog"), j("real-bench")
    ran_r, r_names, r_failed = tests_of("R")
    if ran_r:
        real = {
            "conditions": "143 real IAU CCSIT sheets (restricted personal data, never committed); "
            "reference labels agent-adjudicated (two readers plus the prototype, disagreements "
            "adjudicated on 600-dpi zooms), not certification-grade; real_iau_e2e grades with "
            "inferred keys (--infer-key: accuracy against the reference does not depend on the "
            "key); real_iau_scored grades with the instructors' verified keys and key sheets "
            "(OMR_IAU_KEYS); real_iau_remark is the internal parallel run with Remark Office OMR "
            "(OMR_REMARK_DIR), recorded as pass or fail only",
            "not_proven": "identity against a real roster (all runs rosterless) and the "
            "instructors' own confirmation of their keys in the app; the parallel run with Remark "
            "Office OMR is an internal development check whose head-to-head figures are not "
            "recorded here (spec §3.5, Q-08: the licence review is open)",
            "full_pipeline": pick(e2e or {}, "pages", "questions", "auto_accepted",
                                  "silent_errors", "review", "ar", "student_number", "form",
                                  "blank_pages", "orientation_margin_min", "wall_s",
                                  "pages_per_s"),
            "recognition_pooled_batch": None,
            "bench": bench_summary(bench),
            "tests": r_names,
            "tests_failed": r_failed,
            "pass": not r_failed and len(r_names) > 0,
        }
        if e2e and e2e.get("questions"):
            k = e2e.get("silent_errors", 0)
            real["full_pipeline"]["ser_per_question"] = k / e2e["questions"]
            real["full_pipeline"]["ser_per_question_upper95"] = upper95(k, e2e["questions"])
            real["full_pipeline"]["ser_per_auto_accepted_upper95"] = upper95(
                k, e2e.get("auto_accepted", 0))
        if recog:
            d = recog.get("decisions_pooled_batch", {})
            real["recognition_pooled_batch"] = pick(
                d, "pages", "questions", "auto_accepted", "silent_errors",
                "ser_upper_95_rule_of_three", "review", "ar", "student_number", "section",
                "version", "exam")
            reg = recog.get("registration", {})
            real["registration"] = pick(reg, "failed_pages", "rms_px", "matched")

    def sh(*cmd):
        try:
            return subprocess.run(cmd, capture_output=True, text=True, check=False).stdout.strip()
        except OSError:
            return ""

    def source_digest():
        """Which code this run built (CR-20): the working tree, dirty or not, staged into a
        throw-away index, and the SHA-256 of its `git ls-tree -r` listing of crates/,
        Cargo.toml and Cargo.lock without Markdown files (documentation edits after the run do
        not change it). Once committed, `verify` below prints the same digest exactly when the
        commit holds the code that was measured."""
        idx = os.path.join(a.work, "provenance.index")
        env = dict(os.environ, GIT_INDEX_FILE=idx)
        paths = ["crates", "Cargo.toml", "Cargo.lock"]
        try:
            os.remove(idx)
        except OSError:
            pass
        try:
            subprocess.run(["git", "read-tree", "HEAD"], env=env, capture_output=True, check=True)
            subprocess.run(["git", "add", "-A", "--", *paths], env=env, capture_output=True,
                           check=True)
            tree = subprocess.run(["git", "write-tree"], env=env, capture_output=True, text=True,
                                  check=True).stdout.strip()
            listing = subprocess.run(["git", "ls-tree", "-r", tree, "--", *paths],
                                     capture_output=True, text=True, check=True).stdout
            lines = [l for l in listing.splitlines() if not l.endswith(".md")]
            body = "".join(l + "\n" for l in lines)
            return {"code_sha256": hashlib.sha256(body.encode()).hexdigest(),
                    "files": len(lines),
                    "verify": "git ls-tree -r <commit> -- crates Cargo.toml Cargo.lock | "
                    "grep -v '\\.md$' | sha256sum"}
        except (OSError, subprocess.CalledProcessError):
            return {}
        finally:
            try:
                os.remove(idx)
            except OSError:
                pass

    cpu = ""
    try:
        with open("/proc/cpuinfo", encoding="utf-8") as f:
            cpu = next((l.split(":", 1)[1].strip() for l in f if l.startswith("model name")), "")
    except OSError:
        pass
    # The verification rounds recorded after earlier runs (real_data.*_round) are carried
    # forward: a rerun of this script must not drop them. Without real-data rows in this run,
    # the previous real_data section is carried forward whole, and says so.
    try:
        with open(a.out, encoding="utf-8") as f:
            previous = json.load(f).get("real_data") or {}
    except (OSError, ValueError):
        previous = {}
    rounds = {k: v for k, v in previous.items() if k.endswith("_round")}
    if real is None and previous:
        real = dict(previous)
        real["carried_forward"] = ("not rerun by this acceptance run (no real-data location "
                                   "given): the previous run's real_data section, unchanged")
    elif real is not None:
        real.update(rounds)
    report = {
        "generated_utc": a.start,
        "what": "MVP acceptance run (docs/IMPLEMENTATION.md §1): one object per row; aggregates "
        "only; synthetic data unless the real_data section is present",
        "git": {"head": sh("git", "rev-parse", "--short", "HEAD"),
                "dirty": bool(sh("git", "status", "--porcelain")),
                "measured": source_digest()},
        "machine": {
            "cpus": os.cpu_count(),
            "cpu_model": cpu,
            "os": platform.platform(),
            "load_average_start": a.load_start,
            "load_average_end": a.load_end,
            "note": "shared 4-vCPU container; timings are lower bounds of what a quiet machine "
            "gives when other builds run alongside",
        },
        "toolchain": sh("rustc", "--version"),
        "rows": rows,
        "common_cases": common,
        "adversarial": adversarial,
        "real_data": real,
    }
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(tidy(report), f, indent=2, ensure_ascii=False)
        f.write("\n")
    bad = [r["id"] for r in rows if r["pass"] is False]
    bad += [c["id"] for c in common if c["pass"] is False]
    if adversarial is not None and not adversarial["pass"]:
        bad.append("adversarial")
    if real is not None and real.get("pass") is False and "carried_forward" not in real:
        bad.append("real_data")
    print(f"acceptance_report: wrote {a.out}; rows failing: {bad or 'none'}")
    return 3 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
