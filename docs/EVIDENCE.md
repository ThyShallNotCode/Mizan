# Evidence: the Mizan MVP against its acceptance rows

This document is for Imam Abdulrahman Bin Faisal University (IAU): the people who decide
whether Mizan can grade the university's multiple-choice exams, and the quality
reviewers who rely on its results for NCAAA and ABET reports. It reports what was measured
against the MVP's acceptance rows (`docs/IMPLEMENTATION.md` §1), how each number was
obtained, whether the row passes, and what each number does **not** show.

Every number below comes from one run of `scripts/acceptance.sh` on the code after the
security and code review fixes (§3.2), with the real-data rows and the desktop rows
(`OMR_ACCEPT_A11=1`). That run wrote
[`docs/evidence/acceptance.json`](evidence/acceptance.json), which holds the same figures with
the name of every test that proves each row. Unless a section says otherwise, the data are
**synthetic**: sheets generated with known answers, so the truth is exact by construction.
The 143 real IAU answer sheets are reported separately in §4, as aggregates only. The rows
measured after the acceptance run (scoring with the instructors' keys, the key sheets, the
photographed and damaged sheets, flag stability: §1's last three rows, §3.1.1, §4.1) come from
targeted reruns by the second adversarial verification on the code after its fixes; they are
recorded in `acceptance.json` under `real_data.second_round` with the digest of that code. A third
round (§3.1.2: the mis-key release gate, flag stability by content, photos, key-sheet provenance)
reran the real-data rows, the photo suites and the throughput of the real PDFs on its code
(`real_data.third_round`). The other A1–A11 rows were not rerun (the full script needs a release
rebuild of every crate and about an hour on this shared machine); the recognition engine changed
since, but on the 143 real pages every measurement and decision is bit-identical (§4).

## 1. Summary

| Row | Requirement | Measured | Target | Result |
|---|---|---|---|---|
| A1 | Recognition accuracy | Standard slice: **0 silent errors in 50,000** held-out questions (SER bound 6.0 × 10⁻⁵), AR **0.50 %**. Hard slice: 1 in 50,000, AR 5.9 % (reported, not certified) | 0 silent errors on ≥ 50,000 questions, AR ≤ 1 % | Pass |
| A2 | Throughput, 4 threads | **131.4 pages/s** (own forms, 200-dpi JPEG); **88.4 pages/s** (IAU form, 600-dpi bitonal PDF); a 500-page PDF through the whole application **81.9 pages/s**, peak memory +276 MiB | ≥ 40 pages/s (stretch 100) | Pass |
| A3 | Time to first result | 30 sheets end to end: **0.69 s** (own forms), **0.72 s** (IAU form) | < 3 s | Pass |
| A4 | Regrade latency, 10,000 × 100 | Key change **12.8 ms** (scoring engine); in the app: review action **22.1 ms**, key change **17.1 ms**; a review action including its audited snapshot save **32.6 ms** | ≤ 50 ms | Pass |
| A5 | Determinism | Decision files byte-identical at 1, 2 and 4 threads and in shuffled order (own forms and IAU form) | Byte-identical decisions | Pass |
| A6 | Identity | 21 oracle tests; all 14 IDN-004 sub-reasons and IDN-005 raised where they should be | Correct codes | Pass |
| A7 | Page code | 14 tests: round trip, tampering, forged and foreign sheets → IDN-002 | Tag verified; IDN-002 on failure | Pass |
| A8 | Scoring | 17 oracle tests equal hand computations | Exact match | Pass |
| A9 | Outcomes (CLO/PLO, TP-154) | 12 oracle tests equal hand computations | Exact match | Pass |
| A10 | Exceptions | 0 unknown codes in 241 source files; UNREADABLE never EMPTY in 6 targeted tests | Catalogue test + targeted tests | Pass |
| A11 | Desktop UI | 23 tests (13 on demo data, 10 over the real core: review, release, New exam (with key sheets too), key replacement, key sheets read in Setup and confirmed, flag stability, live grading); screenshots of every screen in both themes, the key-sheet screens included | Screens render; scripted review completes | Pass |
| — | Scoring of the real IAU exams (measured after the acceptance run, when the instructors' keys arrived) | With the real keys: **137 of 143** booklets scored, **each equal** to an independent count of the reference answers against the key (CS 522: 88 of 88, ARTI 503: 49 of 49); the other 6 are provisional and differ only by their 7 questions still in review (a pending question is never credited); **0 of 4,089** auto-accepted answers differ from the reference; forms wrong 0 of 141 | Scores equal those from the real key | **Pass** (`real_iau_scored.rs`, §4.1) |
| — | Key sheets (CR-11; measured after the acceptance run) | The instructors' own key sheets read by the same key-blind recognizer: ARTI 503 (600-dpi scans) **56 of 56** answers and CS 522 (phone photos) **58 of 58** equal to the verified key, none wrong, none to decide; graded with the key sheets named, or found inside the students' stack, and confirmed: the **7 decision files byte-identical** to the key-file run for each exam. Adversarial cases (a blank, multi-marked or erased-and-changed row, a blank version bubble, two sheets that disagree, a key sheet mid-stack, a student who bubbles the marker, a correction after confirmation followed by a rescan) each held, highlighted or refused as designed; 4 defects found and fixed (§3.1.1). **Third round** (§3.1.2): 0 of 58 photo rows highlighted (48 before); each row crop shows its printed question number; every key sheet of a version found in the stack is named, so a full-marks student who bubbled the marker is never dropped silently; key sheets added from the desktop or `omr key replace --from-key-sheets` are stored and named by the key version, own forms included | A wrong key is never used; key-sheet answers never accepted on their own | **Pass** (`real_iau_scored.rs`, `omr-cli/tests/real_key_read.rs`, `omr-app/tests/key_sheets.rs`) |
| — | Photographed and damaged sheets (F1 and the adversarial verification after it) | **F1, found by the owner's run:** both CS 522 key photos registered one bubble row too high and all 58 answers were auto-accepted, 49 of them wrong (reproduced on the previous engine: 447 and 580 of 687 bubbles matched, student number and version wrong). **Fixed** by a shift-sensitive verification against the blank form: both photos now read 58 of 58; 200 simulated photos and 10 adversarial photo profiles (upside down, 11–13° skew, 10–14 % anisotropy, very dark, overexposed, 100 ppi, key sheets) 0 silent errors. **Found in this verification:** a fold, a taped tear or an ADF feed stall moving part of a sheet by 2–3 rows was read rows off and auto-accepted; now refused (row and column checks), 0 silent errors on scanner pages and photos. **Third round:** a photo is recognised as one from its file (a colour JPEG below 200 ppi, a camera's EXIF) or its geometry, and because the phone slice is not calibrated every answer on a photographed student sheet goes to review (never auto-accepted); the key photos read 58 of 58 with 0 rows highlighted (48 before). The 143 real pages read bit-identically | Never silently wrong | **Pass** (§3.1.2, §7) |
| — | Mis-key release gate (§12.2, §8.1; third round) | A probable mis-key at normal confidence (SCO-001 ERROR) is a review item that blocks release until the instructor changes the key, accepts a second answer, voids the item, or keeps the key with a reason and a note; low-confidence flags never block. Flag stability is seeded from the content (identical through two key histories), with every item's bootstrap CI of r_ir and a Monte Carlo margin around 50 %. On the real exams: CS 522 10 rank 1–4 flags, ARTI 503 8, all low confidence or insufficient data (the one previously confident CS 522 flag fired in 51–52 % of resamples, inside the Monte Carlo noise), so no mis-key item is queued | A confident mis-key is never released unacknowledged | **Pass** (`omr-app/tests/flag_stability.rs`, §3.1.2) |

Real data (143 IAU sheets, §4): **0 silent errors in 4,089 auto-accepted answers**, review rate
**0.171 %**, student numbers **0 wrong** (138 exact, 5 sent to review).

Adversarial verification (§3.1): 15 hostile-input tests through the whole application, all
passing; 11 defects found on the way were fixed, each with a regression test. A second round,
after the instructors' keys arrived, verified the photo registration fix (F1), the key sheets and
flag stability: 6 more defects found and fixed, each with a regression test that fails on the
code before its fix (§3.1.1). A third round found that a confident mis-key never reached the
release gate, and eleven more issues (photos, key-sheet provenance, flag stability, remedies,
documentation); all are fixed, each code fix with a regression test (§3.1.2). A fourth round
verified the Remark import and the parallel run: ten defects fixed, each with a regression
test (§5.1); the real-data figures of that run stay inside the university (§5). A fifth round
(§5.2) made the parallel run compare the exam's own grades, queue its differences so they hold
release, refuse points the export cannot pin down and AND keys read as "either", let imported
CLO mappings be approved or replaced without grading again, and moved every expectation tied to
the real exams out of the repository. A review of the G-3 engine (§3.1.4–§3.1.6) fixed ten
defects, among them typed versions that copied a one-column key to every version unasked, an
empty cell of a one-column key that moved every later answer up a question, headerless key rows
the proposal and the definition read two ways, and a cancelled append after which the same
scans could never be added; each fix has a test that fails without it, and the proposal's and
the survey's rules that no test covered have one now.

Security and code review (§3.2): of 26 findings above "low", 21 are fixed (18 with regression
tests; CR-05, CR-06 and CR-08 were documentation), 3 in part (SEC-11, CR-09, CR-15: the rest is
listed in §7), 1 was deferred at the acceptance run and is built since (CR-11, a
teacher-filled key sheet, §4.1) and 1 did not reproduce
(SEC-03); 5 of the 12 low findings are fixed in whole or in part.

**Terms.** A *silent error* is an answer the software accepted on its own that differs from
the truth. *SER* is silent errors per question. *AR* (abstention rate) is the share of
questions sent to a person for review. The *bound* is the one-sided 95 % upper limit on the
true SER (spec §23.1): with zero errors in *n* questions it is 3/*n*. A row "passes" when every
test that proves it passed in the acceptance run.

## 2. The acceptance rows

### 2.1 A1: recognition accuracy with abstention

**What was measured.** Whether the software reads the university's own generated answer sheets
without silent errors, and how often it asks a person instead.

**How.** `omr-sim` generated 500 sheets of 100 questions (5 options, 2 forms, 9-digit ID grid):
simulated pencil and pen fills, light marks, erasures, multiple marks, rotation, blur, noise,
uneven light and paper levels, then a 200-dpi gray JPEG at quality 70–95. The seed
(20 261 016) had never been used while thresholds were tuned (tuning used seeds 1 and 3), so
these are **held-out** questions. Every page went through the real decoder, registration, page
code check and decision rules. The test compared each answer with the truth.
Tests: `omr-recog/tests/ownform_synth.rs::req_a1_ownform_standard_slice_50k_held_out_questions`
and `…::req_a1_ownform_hard_slice_reported_separately`, plus the committed small corpora
(`req_a1_ownform_standard_slice_zero_silent_errors`, `req_a1_ownform_two_page_booklets_route_by_page_code`),
the IAU legacy form (`omr-recog/tests/synthetic.rs::req_a1_synthetic_decisions_follow_the_brief`,
`req_a1_bitonal_600dpi_page_is_reduced_and_flagged`, `fractional_dpi.rs::req_a1_iau_form_at_300_dpi_…`),
the bitonal gate (`ownform_gates.rs::req_a1_bitonal_capture_uses_bitonal_thresholds`) and the
whole application (`omr-app/tests/synthetic_e2e.rs::req_a1_a8_synthetic_sheets_grade_end_to_end`).

| Standard slice (held out) | Pooled batch baseline | Single-page baseline |
|---|---|---|
| Questions | 50,000 | 50,000 |
| Auto-accepted | 49,750 | 49,744 |
| Silent errors | **0** | **0** |
| SER bound (one-sided 95 %, 3/n) | **6.0 × 10⁻⁵** | 6.0 × 10⁻⁵ |
| SER bound at the spec's clustering floor (DEFF 2) | 1.2 × 10⁻⁴ | 1.2 × 10⁻⁴ |
| Review rate (AR) | **0.50 %** | 0.51 % |
| Sheets with a silent error | 0 of 500 (bound 0.6 %) | 0 of 500 |
| Review-only marks auto-accepted | 0 | 0 |
| Student IDs | 500 exact, 0 wrong | 500 exact, 0 wrong |
| Registration | 500 / 500 pages, page code verified on all; worst bubble-centre error 0.10 px | |

The 250 reviews on the standard slice are erasures whose residue is about as dark as a light
fill (135), light marks (47), ambiguous marks (35), multiple marks (25) and others (8).

**Hard slice, reported separately.** Same seed and size, harder profile: light fills, erasure
residue, ticks and cross-outs, ±4° rotation, blur, dirty-glass streaks. 50,000 questions,
47,057 auto-accepted, **1 silent error** (SER 2.0 × 10⁻⁵, bound 9.5 × 10⁻⁵), AR 5.9 %. No tick
or cross-out was auto-accepted. The one error is a two-option answer (B and D) read as B alone:
the student's second fill was as faint as erasure residue. Where the simulator's light-fill
and residue darkness ranges overlap, the classical rules cannot tell the two apart. Only the
learned mark classifier, which the MVP defers (spec §8.7), can.

**Result: pass.** 0 silent errors on 50,000 held-out questions, AR 0.50 % (target ≤ 1 %).

**Caveats.**
- This is a **semi-synthetic** slice. The marks and scanner effects come from a simulator, and
  the thresholds were tuned on the same simulator (other seeds). Real pencil on real paper can
  differ; §4 shows the real IAU sheets.
- 50,000 questions bound SER at 6 × 10⁻⁵, not the 1 × 10⁻⁵ of the general-availability
  contract (spec §2.4, §23.1). Certifying that needs about 600,000 test questions plus 600,000
  for the calibration split (spec §23.2: at least 13,000 sheets); a field claim needs the
  university's own labelled sheets.
- AR 0.50 % sits at the spec's general-availability limit (0.5 %), inside the MVP's 1 %.
- The hard-slice error is a known limitation until the mark classifier ships.

### 2.2 A2: throughput

**What was measured.** Pages recognised per second at 1, 2 and 4 threads: decode, page stage
(registration, page code, measurement) and batch stage (baselines, decisions). The full
in-memory pipeline (identity, form, scoring, review queue) is reported too.

**How.** `omr bench` (release build, best of 3) on two corpora written to disk:
240 own-form sheets (200-dpi gray JPEG) and 150 synthetic IAU sheets on the committed blank
IAU form, delivered as the university's Canon scanners deliver them: 600-dpi bitonal PDFs of
30 pages. Tests: `omr-cli/tests/acceptance_perf.rs::req_a2_throughput_ownform_200dpi_jpeg_corpus`
and `…::req_a2_throughput_legacy_iau_bitonal_pdf_corpus`.

| Corpus | 1 thread | 2 threads | 4 threads | Full pipeline, 4 threads |
|---|---|---|---|---|
| Own forms, 200-dpi JPEG | 34.9 | 65.4 | **131.4** | 132.9 |
| IAU form, 600-dpi bitonal PDF | 22.2 | 40.9 | **88.4** | 78.6 |
| Real IAU PDFs (143 pages, §4) | 19.4 | 36.9 | **73.2** | 67.8 |

(pages per second)

**Large input.** One scanner PDF of 500 own-form pages (`omr-sim`, 200-dpi JPEG pages, 223 MiB)
graded through the whole application at 4 threads, the audited store included
(`omr-app/tests/adversarial.rs::req_a2_a_500_page_pdf_keeps_memory_bounded_and_throughput`):
**81.9 pages/s** (6.1 s; the page stage alone 142.8 pages/s), 0 silent errors in 19,886
auto-accepted answers. Peak memory grew by **276 MiB**: the PDF (223 MiB) held once, plus
about 100 KB of measurements per page. Keeping the decoded pages would have cost about
1.9 GB; before the adversarial fixes the file was held twice at the peak (+447 MiB).

**Result: pass.** Every 4-thread figure is above the 40 pages/s floor.

**Caveats.** The machine was a shared 4-vCPU container (§6), not one of the spec's reference
laptops, and its speed varies with the load: earlier acceptance runs of this code base gave
125–134, 83–98 and 73–89 pages/s at 4 threads for the three corpora. The load average was
3.4–3.9 during the throughput rows of this run, partly from the acceptance run's own
preceding tests.

**Re-measured after the third round** (§3.1.2, C9) on the real IAU PDFs only: `omr bench` (best
of 2) with the binary of the code measured here and the binary of the acceptance run, three
alternating rounds on the same machine at a load average of 0.9–2.1:

| Real IAU PDFs, 143 pages | 1 thread | 4 threads | Full pipeline, 4 threads |
|---|---|---|---|
| Acceptance-run code | 17.6–19.3 | 58.9–69.2 | 63.0–71.0 |
| Code after the third round | 17.1–17.5 | **61.0–64.1** | 62.9–64.3 |

(pages per second; the range of the three rounds.) The two overlap within the machine's noise,
and both are above the 40 pages/s floor. They are lower than the table above because the
machine was busier than in the acceptance run, as the old binary shows.

### 2.3 A3: time to first result, 30 sheets

**What was measured.** How long a teacher waits from starting a grade of 30 sheets until every
result file exists.

**How.** The wall time of the `omr grade` process on exactly 30 sheets at 4 threads: process
start, read, safe copy of the originals, recognition, identity against a roster, scoring with
a key, CLO attainment, item analysis, the audited project store and every export.
Test: `omr-cli/tests/acceptance_perf.rs::req_a3_time_to_first_result_30_sheets`.

| Corpus | `omr grade`, 30 sheets | `omr bench` in-process 30-sheet run |
|---|---|---|
| Own forms, 200-dpi JPEG | **0.69 s** | 0.81 s |
| IAU form, 600-dpi bitonal PDF | **0.72 s** | 0.55 s |
| Real IAU PDFs | — | 0.59 s |

**Result: pass** (< 3 s). **Caveat:** same machine conditions as A2.

**Re-measured after the third round** (C9, the alternating rounds of §2.2): the in-process
30-sheet run of the real PDFs took **0.71–0.93 s** (first provisional page after 218–237 ms),
against 0.58–0.65 s (136–165 ms) for the acceptance-run binary in the same rounds: the code
added since the acceptance run (two rounds of checks and the flag stability in the exports)
costs 0.1–0.3 s per run here, not profiled further; the row stays well inside 3 s.

### 2.4 A4: regrade latency

**What was measured.** How long a change takes to show in the results of an exam with 10,000
students and 100 questions.

**How.** Release build, best of 5. In the scoring engine
(`omr-scoring/tests/regrade.rs::req_a4_regrade_10k_x100`): ten items re-keyed on four permuted
forms, with validation, key compilation, item scoring, adjustments, checks, a new run ID and
the Saudi grade scale. In the application (`omr-app/tests/synthetic_e2e.rs::req_a4_rescoring_is_incremental_and_fast`):
the session's incremental rescoring after one review decision and after a key change; the test
also checks that the incremental result equals a full rebuild. The same test then times a
review decision as the application performs it: the rescoring plus the save of the session
snapshot (serialised, SHA-384, synced to disk and bound to the audit chain by its
`SnapshotSaved` event), for the same 10,000 booklets.

| Path | Time | Budget |
|---|---|---|
| Scoring engine, key change (10 items, 4 forms) | **12.8 ms** | 50 ms |
| Application, one review decision | **22.1 ms** | 50 ms |
| Application, key change | **17.1 ms** | 50 ms |
| Application, one review decision with its snapshot save (156 MB snapshot) | **32.6 ms** | reported (asserted ≤ 250 ms) |

**Result: pass.** **Caveats:** the 50 ms budget applies to the rescoring paths. The snapshot
save stays near them because a decision rewrites only the snapshot's header and the one chunk
of 64 booklets it changed; when the whole snapshot was rewritten, the same decision took
about 0.9 s at this size. A command-line decision also opens the project with its full audit
checks, signs a checkpoint and re-exports every file: on the real CS 522 exam (92 pages) a
whole `omr review resolve` process took 100–123 ms, 67–78 ms with `--no-export`, and its undo
95–136 ms (three runs each, after the acceptance run). On a large exam, resolve with
`--no-export` and run `omr export` once at the end; the desktop app keeps the project open and
exports only on request.

### 2.5 A5: determinism

**What was measured.** Whether the same scans always give the same decisions, whatever the
number of threads or the order of the files.

**How.** Own forms (`omr-app/tests/synthetic_e2e.rs::req_a5_outputs_identical_across_threads_and_input_order`):
24 sheets graded five times (1, 2 and 4 threads in name order; 4 threads in a seeded shuffle;
2 threads reversed). IAU form (`omr-app/tests/acceptance.rs::req_a5_legacy_decisions_identical_at_1_2_4_threads_and_shuffled_order`):
12 PNG sheets plus a 600-dpi bitonal PDF of 6, graded four times. Every exported file is
compared byte for byte with the 1-thread run, together with every page, booklet and review
item. Engine level: `omr-recog/tests/ownform_determinism.rs::req_a5_ownform_identical_across_threads_and_page_orders`
(measurements identical at 1, 2 and 4 threads) and `omr-recog/tests/synthetic.rs::req_a5_page_stage_is_deterministic`.

**Byte-identical in every run:** `results.csv`, `item_analysis.csv`, `test_stats.csv`,
`tp154_a1.csv`, `tp154_b1.csv`, `booklets.json`, `review_queue.json`, `key.csv` and
`blackboard_grade_center.csv`: every answer, score, grade, identity, form and queued item.
(The IAU-form exam has no roster or CLO map, so it writes no `tp154_b1.csv` or Blackboard
file; all its other decision files are compared.)

**Excluded, and why.** The comparison leaves out six files, and they do differ between runs:
- `results.xlsx`, `item_analysis.xlsx`, `tp154.xlsx` and `report.html` print the head of the
  audit chain in their metadata. The chain hashes each event with the time it was recorded, so
  the head changes with the wall clock even when every decision is the same.
- `summary.json` reports the run's timings.
- `manifest.json` lists the SHA-384 of every export, so it changes with the files above.

The project store itself (the SQLite event log) also records wall-clock times and is not a
decision file.

The adversarial suite adds a hostile batch (rescans, byte-identical copies, empty and
truncated files, a multi-page TIFF, a blank page, 150- and 75-dpi scans) graded at 1, 2, 3 and
4 threads in name, reversed and shuffled order: decision files byte-identical
(`omr-app/tests/adversarial.rs::hostile_batches_are_deterministic_across_threads_and_input_order`).

**Result: pass.** **Caveat:** one machine and one CPU type. The spec's cross-ISA check (CI-3)
is not part of the MVP.

### 2.6 A6: identity

**What was measured.** Whether each sheet is matched to the right student, and whether every
doubtful case is flagged with the right code rather than guessed.

**How.** 20 oracle tests in `omr-identity/tests/a6_identity.rs` (all `req_a6_…`) and
`omr-recog/tests/ownform_gates.rs::req_a6_unverified_page_id_is_never_exact`: exact roster
match from the bubbled ID; blank, double and unreadable columns as wildcards; **never snapping
to the nearest roster entry** (property test); national ID and Iqama detection
(`wrong_identifier`, digits masked); typed-ID agreement and conflict; rosterless mode;
duplicates (**IDN-005**) and not-on-roster; assignment invariants under booklet order and
roster changes. All 14 IDN-004 sub-reasons of Appendix A are raised and covered. On the IAU
form through the whole app, the common case C10 adds a blank ID column, two bubbles in one
column, an ID on no roster entry and two sheets with one ID (§3).

**Result: pass.** **Caveats:** identity is certified per booklet in the spec (§23.1: about
30,000 labelled auto-accepted sheets), which no MVP run reaches. The handwritten-name channel
is deferred. Rosterless identities that need confirmation are not queued, and the review
screen has no "duplicate" decision yet.

### 2.7 A7: page code

**What was measured.** Whether each own-form page proves which exam, form, booklet and page it
is, and whether a forged or foreign sheet is stopped.

**How.** `omr-forms` unit and property tests (`req_a7_round_trip_and_length`,
`req_a7_wrong_key_is_rejected`, `req_a7_tampered_body_is_rejected`,
`req_a7_non_zero_pad_is_rejected`, `req_a7_encode_decode_round_trip`,
`req_a7_any_single_character_change_is_rejected`, `req_a7_other_keys_never_verify`); rendered
sheets readable at 200 and 300 dpi (`tests/render.rs`); every copy outcome
(`omr-recog`: `req_a7_copy_outcomes`, `req_a7_page_code_copy_outcomes_on_rendered_sheets`,
rotated and mirrored pages); and, new for this report,
`omr-app/tests/acceptance.rs::req_a7_forged_and_foreign_sheets_are_held_with_idn002`: a sheet
printed with the exam's layout but another key, and a sheet of another exam, are graded in one
batch with genuine sheets. Neither is attached to a booklet; each is held with **IDN-002
(BLOCKER)** in the Capture lane and release is blocked, while the genuine booklets read.

**Result: pass.** **Caveats:** the page-code secret is stored in an owner-only file of the
project (`secrets/`), not encrypted (encryption at rest is deferred), and a page whose code
cannot be verified cannot yet be bound to a booklet by hand: it goes to the rescan list. Each
exam's sheets get their own exam ID and key (`omr sheet render` and `omr templates
--example-spec` never reuse one), and a second project of the same exam is refused unless asked
for (`--reuse-exam-id`).

### 2.8 A8: scoring

**What was measured.** Whether scores, percentages and letter grades equal hand calculations.

**How.** 15 oracle tests in `omr-scoring/tests/a8_oracles.rs` (multi-form mapping, item rules,
formula scoring, exact rationals such as 0.3 points, the Saudi grade scale, course split and
status codes, publish validation, unreadable and absent answers, voids, bonus items, choice
groups, floors and penalties, not-reached rows, key revisions, score boxes, malformed input),
the golden results CSV (`omr-report/tests/golden.rs::req_a8_results_csv_letters_on_the_saudi_scale`)
and the end-to-end test, where every score, percentage and Saudi letter of 24 graded booklets
equals a computation from the truth and the key.

**Result: pass.** **Caveat:** permuted forms with a declared item and option map (π/σ) are not
yet accepted from the exam definition, so forms with different keys are scored as separate
item sets.

### 2.9 A9: outcomes (CLO/PLO, NCAAA and ABET tables)

**What was measured.** Whether CLO and PLO attainment and the NCAAA TP-154 tables equal hand
calculations.

**How.** 11 oracle tests in `omr-outcomes/tests/a9_oracles.rs` (student-performance worked
example, average and threshold-proportion methods with EAMU levels, Taif per-question,
section pooling and suppression, PLO roll-up, exit-exam item maps, TP-154 B.1 rows and A.1
distribution, frameworks and the ABET crosswalk, exceptions and missing data) and the golden
TP-154 CSV with bilingual headers (`omr-report/tests/golden.rs::req_a9_tp154_a1_and_b1_csv_with_bilingual_headers`).

**Result: pass.** **Caveat:** the MVP writes the TP-154 tables as CSV, XLSX and HTML; filling
the official NCAAA DOCX forms is deferred.

### 2.10 A10: exceptions

**What was measured.** Whether every exception code the software can raise is one of the
spec's catalogue (Appendix A), and whether an unreadable answer is never treated as blank.

**How.**
- `omr-model/tests/catalogue.rs::req_a10_every_code_in_the_workspace_sources_is_in_appendix_a`
  scans every `.rs` and `.slint` file of the workspace (241 files, 1,213 code
  occurrences): **0 unknown codes** (the one exception is the deliberate `MRK-999` of the
  catalogue's own negative test), and **0 wrong severity or scope** at the 99
  constructor calls with literal arguments. The other 8 calls compute their arguments and are
  checked when they run. All 115 Appendix A codes appear in the sources. Separately,
  `catalogue_matches_appendix_a` checks that the generated catalogue equals Appendix A.
- UNREADABLE is never EMPTY: a cut-off scan (`omr-recog/tests/synthetic.rs::req_a10_cut_off_bubbles_are_unreadable_never_blank`),
  a page that does not register (`req_a10_failed_registration_is_unreadable_never_blank`), own
  forms cut half way or blank (`ownform_identity.rs::req_a10_blank_and_cut_off_pages`), pages
  with two tags (`ownform_gates.rs::req_a10_two_tag_pages_are_never_auto_graded`), identity
  codes (`a6_identity.rs::req_a10_identity_exceptions_are_catalogued`), corrupt files
  (`synthetic_e2e.rs::req_a10_corrupt_inputs_inferred_key_and_view_models`), and, new,
  `omr-app/tests/acceptance.rs::req_a10_cut_off_answers_stay_pending_never_scored_blank`: a
  100-question IAU sheet with its right edge cut off has questions 81–100 `Unreadable`
  (CAP-009), each queued for review, and the student's total stays provisional until a person
  decides.

**Result: pass.** **Caveats:** sub-reasons are checked by tests, not by the scan. A sticker or
white-out over one *unfilled* bubble on the IAU form still reads as blank: ring visibility
(spec §8.6) is checked per answer row only (since the second adversarial round a row whose
printed rings are not seen is unreadable, §3.1.1), not per bubble. WARN and INFO exceptions are
counted but not queued.

### 2.11 A11: desktop UI

**What was measured.** Whether every screen of the desktop app renders, and whether a person
can do the whole job through it: create an exam, follow grading, review, replace the key and
release.

**How.** `scripts/acceptance.sh` with `OMR_ACCEPT_A11=1` ran both A11 test files (in the dev
profile: the row is not timed):
- `crates/shell-slint/tests/req_a11.rs` (13 tests, demo backend): the review flow through
  accessible labels and physical keys, identity candidates, the setup matrix, unique accessible
  labels, every screen in both themes at 1440 × 900 and 1280 × 800, keyboard paths, results and
  outcomes, and the DEMO DATA mark on every screen.
- `crates/shell-slint/tests/req_a11_real.rs` (10 tests over the real `omr-app` backend, synthetic
  IAU sheets): a scripted review that rescores the results exactly as the core does, then
  every item settled and the results released through *Release results…* with the Blackboard
  upload; live
  grading; adding a later batch from the Scan screen; edge projects (no roster, no CLOs, an
  inferred key) and an empty workspace without a panic; **New exam** from the dashboard (a bad
  entry refused with what to type, then graded into a new folder of the workspace with an
  inferred key, and opened; and again with the instructor's key sheets in its *Key sheets* field:
  read with the exam as stored key sheets, held, confirmed in Setup, the key version naming both
  scans, §3.1.2); **Replace key…** from Setup (the inferred key replaced and the exam
  rescored); **Read key from answer sheet…** in Setup (a student's sheet refused, the
  instructor's key sheets previewed, then confirmed: since the third round they join the exam
  as stored, audited key sheets, §3.1.2); flag stability in Results' item analysis; and a
  project whose snapshot was edited outside the app refused with SYS-011 and the remedy.
- The 32 unit tests of the shell (key map, bindings, view-model formatting, flag stability)
  run in `scripts/check.sh`.
- Since that run, the desktop workflow without the command line (G-3) is driven as a user
  drives it, with real pointer clicks and typed keys, by
  `crates/shell-slint/tests/req_gui_workflow.rs` (47 tests, in `scripts/check.sh`, not in the
  acceptance run): file dialogs on every path field, another exams folder opened and read again,
  folders shown in the file manager, Settings kept between starts, the New-exam card (the scans
  checked, every value with its source, a question that blocks Grade exam until answered, the
  definition graded equal to the proposal's), the exports written after a run with the
  completion notice and its next steps, a new exam's grading cancelled, a roster given to a
  graded exam, and the staging of the workflow's screenshots. The window gets the file
  dialogs, the file manager and the preferences from a fake host: the native dialogs were never
  shown (§7).

**Screenshots** (synthetic data only, rendered headless by `omr-desktop --screenshot`):
[`design/screens/`](design/screens/) (the demo data, marked DEMO DATA: exams, exams-new (an
invented card), setup, scan, review, review-identity, review-blind, results, outcomes,
settings; light, dark and 1280 × 800) and [`design/screens/real/`](design/screens/real/)
(synthetic IAU exams graded through `omr-app`: the same screens plus review-scoring,
review-override, results-export, scan-add, the New-exam card as it opens (exams-new), made from
an exam's scans with the answer key asked (exams-new-ask) and from its scans, key, roster and
CLO files with nothing asked (exams-new-ready), the completion notice of a grading run
(scan-notice), an empty exams folder (exams-empty), the key-sheet screens setup-keysheet (the
key summary to confirm, each row with its printed question number) and scan-keysheet, and the
Remark screens), for example [the review screen](design/screens/real/review-light.png),
[results](design/screens/real/results-light.png),
[New exam](design/screens/real/exams-new-ready-light.png) and
[the completion notice](design/screens/real/scan-notice-light.png).

**Result: pass.** **Caveats:** the app has never run in a real window; on Windows it has run only headless, in CI (§7): the
tests use Slint's testing backend and the screenshots its software renderer, and the native
file dialogs, the file manager and the startup message box have never been shown. GUI gap: no
keymap editor. (A roster can be given to a graded exam from Setup's step 4 Roster since G-3,
§3.1.7; the CLO file can be replaced in Setup since §5.2.)

## 3. The common cases (C1–C10)

Each case a teacher meets in practice has a committed test on synthetic data (the committed
blank IAU form with synthetic pencil marks, or `omr-sim` own forms). All passed in the
acceptance run.

| Case | What happens | Tests |
|---|---|---|
| C1 Pages turned 90°, 180° or 270° | IAU sheets are turned upright (REG-002 notes the turn); a page whose orientation cannot be decided is REG-001 and nothing on it is accepted; own forms turn by their corner tags | `omr-app/tests/iau_capture.rs`: `c1_legacy_pages_turned_a_quarter_or_upside_down_are_read_upright`, `c1_legacy_orientation_that_cannot_be_decided_is_never_read_silently`, `c1_own_form_pages_at_90_180_270_degrees_are_read` |
| C2 Blank backs from duplex scanning | Dropped with CAP-012 (INFO), never collated or queued; booklets stay in order | `c2_blank_backs_in_a_pdf_and_as_image_files_are_dropped_with_cap012`, `c2_own_form_blank_backs_do_not_shift_collation` |
| C3 Mixed scans in one exam | A 600-dpi bitonal PDF, 300-dpi JPEGs and 200-dpi PNGs each get their own baseline and read as when graded alone | `c3_bitonal_pdf_gray_jpeg_and_png_in_one_exam_each_get_their_baseline` |
| C4 A later batch (another section or day) | `omr add` regrades everything together; results byte-identical to grading all at once; earlier decisions carry over | `omr-app/tests/append_batch.rs`: `c4_appending_a_batch_equals_grading_everything_at_once`, `c4_review_decisions_taken_before_the_append_carry_over`; `omr-cli/tests/real_world.rs::omr_cli_real_world_cases` |
| C5 Answer keys | The real key replaces an inferred one; a mis-key is corrected with an audited new key version; eight key-file layouts and encodings read the same; a malformed file names its line and the fix | `omr-app/tests/instructor_files.rs`: `c5_key_files_in_every_common_layout_read_the_same_key`, `c5_malformed_key_files_fail_with_the_line_and_what_to_change`; `omr-app/tests/project_actions.rs`: `c5_the_real_key_replaces_an_inferred_key_and_the_blocker_clears`, `c5_a_mis_keyed_answer_is_corrected_with_an_audited_regrade`; CLI test |
| C6 Rosters | BOM, Arabic headers and names, title rows, Excel-damaged IDs repaired only when nothing was lost; duplicates refused naming the rows | `c6_roster_csv_with_bom_arabic_names_and_excel_damage`, `c6_duplicate_and_damaged_roster_ids_are_refused_with_the_fix`, `c6_lost_leading_zeros_restored_only_when_the_scheme_makes_it_unambiguous`, `c6_roster_xlsx_with_numeric_id_cells_and_arabic_names`; CLI test |
| C7 Overriding the software | A decision against the engine's proposal needs a reason code (and a note for OTHER); the reason is in the audit trail | `c7_overriding_the_engine_needs_a_canonical_reason`; CLI test |
| C8 Blackboard upload | Grade-center CSV with username and score only, for final results only; never for an inferred key or a rosterless exam | `c8_blackboard_grade_center_upload_holds_only_what_it_needs`; CLI test |
| C9 Absent, partial and odd sheets | An absent sheet is held (MRK-015), never scored 0; unanswered questions score as blank; stray marks outside the exam's rows are ignored; a row with every bubble filled is a multiple mark, never credited | `omr-app/tests/iau_sheets.rs::c9_absent_partial_stray_and_full_row_sheets` |
| C10 Student-ID mistakes | Blank column, two bubbles in a column, an ID on no roster entry and two sheets with one ID each get their IDN code and a roster suggestion | `c10_id_mistakes_get_idn_codes_and_roster_suggestions` |

### 3.1 Hostile inputs (adversarial verification)

An adversarial pass tried to break the whole application with the inputs below
(`omr-app/tests/adversarial.rs`, synthetic data only). The rule for every case: the right
Appendix A code, never a crash, never a silent wrong score. Eleven defects were found and fixed;
each now has a regression test (marked **fixed**). All tests passed in the acceptance run.

| Input | What happens now | Test |
|---|---|---|
| Zero-byte PDF, JPEG and PNG files | Each is its own CAP-001 (BLOCKER), naming the file and what to do. **Fixed:** the second and third empty file were treated as byte-identical copies of the first (CAP-011, INFO, not queued), so two lost scans went unreported | `broken_files_are_cap001_naming_the_file_and_the_fix` |
| A JPEG cut short (30 %, 90 %, or just its last 2 bytes missing) | CAP-001 "truncated JPEG". **Fixed:** such files were decoded with the missing rows filled with gray, which can read like marks; the spec (§7.3) requires CAP-001 | same |
| A truncated PDF or PNG, an encrypted PDF, a PDF named `.jpg` | CAP-001, each detail naming the file and page, the reason (cut short, password, unsupported) and the remedy. **Fixed:** PDF refusals did not name the file, and no CAP-001 said what to do; `omr grade` now lists every page not graded with its reason and remedy | same; `omr-cli/tests/cli.rs` |
| A multi-page TIFF (a scanner batch in one file) | One page per image, every sheet graded; a damaged page chain grades the pages before the damage and reports the rest as CAP-001. **Fixed:** only the first page was read, and the other sheets disappeared without any exception while the exam read as releasable | `every_page_of_a_multi_page_tiff_is_graded_and_a_damaged_rest_is_cap001` |
| Pages turned 90°, 180°, 270° (image files and PDF `/Rotate`) | Read upright, every answer correct | `turned_pages_read_upright_and_mirrored_ones_are_never_read` |
| Mirrored or upside-down-mirrored IAU sheets | Held (REG-003, ERROR), never read | same |
| Noise, a black page, a text page, a sheet with every bubble filled, an own-form sheet in an IAU exam and an IAU sheet in an own-form exam | Held (REG-001 or IDN-002, BLOCKER) in the Capture lane, never graded; release blocked | `pages_that_are_not_this_exams_sheet_are_held_never_graded` |
| A sheet printed as form C in an exam that declares forms A and B (or only A) | IDN-002 (BLOCKER): held, with the fix (add form C and its key, or remove the sheet). **Fixed:** it was treated as "no form declared", given a provisional form A by key likelihood (margin ≈ 0) and offered to the reviewer as form A | `a_sheet_of_a_form_the_exam_does_not_declare_is_held_with_idn002` |
| JPEG scans at 150, 300 and 400 dpi; 75 dpi | 150, 300 and 400 dpi read exactly (IAU form and own forms, 0 silent errors); 150 dpi carries CAP-002 WARN; 75 dpi is refused with CAP-002 ERROR and queued. **Fixed:** the spec's page-resolution rule (§8.9: below 100 dpi rejected, 100–150 WARN, above 400 INFO) was not applied, and a 75-dpi page was graded | `scans_at_150_300_400_dpi_read_and_below_100_dpi_are_refused_with_cap002` |
| The same sheet scanned twice, a page fed twice inside one PDF, a byte-identical file copy, another sheet carrying the same student number | Agreeing copies are COL-003 (INFO): the copy with fewer abstentions is graded, the other is kept and linked; a byte-identical file is CAP-011; the other sheet with the same number is IDN-005 on both. **Fixed:** rescans of IAU sheets were two booklets with IDN-005 (both counted in the item analysis, and the reviewer had no way to drop one), and own-form rescans were COL-004 "different content" even when the marks agreed | `rescans_and_duplicated_pages_are_col003_when_marks_agree`; `robustness.rs::a_rescanned_sheet_is_col003_when_marks_agree_and_col004_when_they_differ` |
| A sheet with no answers, a sheet with no mark at all, an own-form sheet with every bubble filled | Held as MRK-015 (never scored 0); every question multiple-marked, score 0 | `every_bubble_filled_or_none_marked_is_never_credited` |
| A key with a void (`*`) and an any-of entry (`A\|C`) | Void dropped from the maximum (28 of 29); A or C alone credited, A and C together not, B not | `a_void_and_an_any_of_key_entry_score_as_declared` |
| A key entry left empty | Release blocked with "key incomplete (SCO-003): no key for B-Q4". **Fixed:** the score run was refused but the exam read as releasable | `an_incomplete_key_never_reads_as_releasable` |
| A review decision, then a key change on that question, then undo | Every step rescores; the incremental result equals a full rebuild; undo reverts the decision (not the key); the events are in order and the audit chain verifies; a reopened project holds the same state | `review_then_key_change_then_undo_rescore_and_stay_audited` |
| A batch appended, then undo | Undo of a decision carried over the append is refused (exit 6) and changes nothing; a decision after the append undoes normally | `undo_after_appending_a_batch_stops_at_the_append_and_rescores` |
| A cleaner rescan arriving in a later batch, after a reviewer decided the earlier copy | The reviewer's value is kept and SYS-032 is queued. **Fixed:** the earlier booklet was no longer found, so the re-run's reading replaced the human decision silently | `a_cleaner_rescan_in_a_later_batch_never_silently_replaces_a_reviewers_decision` |
| One student assigned to a whole identity group | Refused (exit 6) and nothing changes. **Fixed:** the first booklet took the student, the second refused, and the first change stayed in the exam without a history record, so undo could not revert it | `a_group_decision_refused_part_way_changes_nothing` |
| All of the above in one batch, at 1, 2, 3 and 4 threads, in name, reversed and shuffled order | Decision files byte-identical | `hostile_batches_are_deterministic_across_threads_and_input_order` |
| One 500-page scanner PDF (`omr-sim`, 223 MiB) | See §2.2: memory bounded, throughput held, 0 silent errors. **Fixed:** every input file was held twice at its peak (read into a buffer, then copied) | `req_a2_a_500_page_pdf_keeps_memory_bounded_and_throughput` |

### 3.1.1 Second round: photos, key sheets and flag stability

After the instructors' keys and key sheets arrived, a second adversarial verification
reproduced the claims of the photo registration fix (F1), the key sheets (CR-11) and flag
stability (F2), then tried to break them. Synthetic sheets only, except where a gated real run
is named; real runs report counts only. Six defects were found and fixed; each regression test
fails on the code before its fix. All tests pass.

| Input | What happens now | Test |
|---|---|---|
| F1 (found by the owner's run): the CS 522 key photos (RGB JPEG ≈ 142 ppi, 4–6 % anisotropic) | Both read their student number, version and 58 of 58 answers equal to the verified key, on the scanner path with a verified one-row correction and on the camera path alone. The previous engine, rebuilt and rerun in this verification: one row too high, 447 and 580 of 687 bubbles matched, **49 of 58 answers wrong and every one auto-accepted**, student number and version wrong on both pages | gated `omr-recog/tests/real_key_sheets.rs`; `register.rs::one_pitch_aliases_are_never_verified_and_are_corrected` |
| Phone photos upside down, skewed 11–13°, 10–14 % anisotropic, very dark (exposure 0.25–0.45), overexposed (1.3–1.8), at 100 ppi, moved one or two pitches in the frame; key sheets bubbled all 0 or all 9 with every question marked; a student number that is mostly zeros | Read or refused (REG-001/REG-003), never misread: 0 silent errors, 0 wrong numbers or versions (exploration: 40–60 photos per profile and combinations; the committed test 6 per profile) | `omr-recog/tests/photo_adversarial.rs::req_a1_adversarial_photos_are_read_or_refused_never_silently_wrong` |
| A fold or a taped tear hiding 2–3 answer rows; an ADF feed stall repeating 1–2 rows' worth of scan lines; on scanner pages, a fold or stall across two options | Refused (REG-003: part of the page moved; rescan it flat) or the hidden rows unreadable; 0 silent errors on scanner pages and photos. **Fixed:** the region-wide verification passed on the part of the page that still matched, and every row below the damage was read two or three rows off, auto-accepted (88–123 silent errors per 16–24 pages in the exploration; 41 in the first 6 pages of the committed test). Each answer row is now compared with the question numbers of the rows around it, each answer column (scanner pages) with the option letters, and a blank-read row whose printed rings are not seen is unreadable. The 143 real pages: every measurement and decision bit-identical | `photo_adversarial.rs::req_a1_folded_or_stalled_sheets_are_refused_never_read_rows_off` |
| A key sheet in the students' stack with its version bubble left blank | On a two-version exam it is refused naming what to do (bubble the version and rescan, or give the key as a file), and the run is held; on a one-version exam it is that version. **Fixed:** it was graded as a student whose number is `0000000000`, its answers in the students' statistics | `omr-app/tests/key_sheets.rs::a_key_sheet_with_a_blank_version_is_never_graded_as_a_student` |
| An answer the instructor erased and changed on the key sheet; a key file given too that says otherwise on a row | The new answer is read; the row is highlighted for checking (MRK-003), as is the row where the key file differs; neither blocks the confirmation. **Fixed:** both rows looked like any other confirmed row | `…::an_erased_and_changed_key_answer_is_highlighted_for_checking` |
| A student who bubbled `0000000000` by mistake, beside the real key sheet | The two key sheets of the version disagree, so those rows wait for the instructor, and the hold now says so and gives the way out: grade again naming the key sheet with `--key-sheet-marker none`, which grades the student. **Fixed:** the hold did not say why the rows disagreed or how to get the student graded | `…::a_student_who_bubbles_the_marker_is_named_by_the_hold_with_the_way_out` |
| A key entry corrected after the key sheets were confirmed (`omr key set`, an audited key version), then the key sheet scanned again in a later batch | The correction stays the instructor's decision: the next key summary shows the row as "decided by you", and the re-confirmed key keeps it. **Fixed:** the rescan dropped the confirmation and the re-confirmed key reverted silently to the sheet's reading | `…::a_key_correction_after_confirmation_survives_a_rescan_of_the_key_sheet` |
| F2: SCO-001 "probable mis-key" on items most of the class answered correctly where one top scorer chose a distractor | The exports run the flag-stability job (§12.2: N < 1,000 → 1,000 bootstrap resamples, seeded from the score run then, from the scored content since the third round (§3.1.2); N ≥ 1,000 → Fisher-z CI ends) and carry each rank 1–4 flag's share and its low-confidence label in `item_analysis.csv`, `report.html` and the XLSX. A genuine mis-key stays confident (share 1.0, both CI ends hold); the one-outlier flag is low confidence (share 0.64, fails at the upper CI end). Identical at 1 and 4 threads. **Fixed:** the export path never ran the job, so every flag read as confident | `omr-app/tests/flag_stability.rs::exported_flags_carry_their_stability_and_low_confidence_label`; `omr-analytics/tests/stability.rs` |
| Key sheet mid-stack in a multi-page PDF; confirmation by two operators (one per version, the project closed and reopened between); undo after confirmation; a key change after confirmation | As designed, no defect: the students' pages and scores are unchanged by the key sheet's place; each version's confirmation records its operator (`confirmed_by`) and the publishing event the last one; undo reverts the latest review decision, never a key confirmation (correct a confirmed key with `omr key set` or `omr key replace`); a key change after confirmation is an audited new key version | exploration; `key_sheets.rs` |

### 3.1.2 Third round: the mis-key gate, photos, key-sheet provenance

A third verification read the code and the evidence of the second round against the spec and
raised twelve findings (C1–C12 of that review; not the common cases of §3). Each was
reproduced first; every code fix has a regression test that fails on the code before it, and
every gated real-data test was rerun in release on the fixed code (aggregates only, §4, §4.1).

| Finding | What was wrong | What happens now | Tests |
|---|---|---|---|
| C1 (high) | A probable mis-key at normal confidence (SCO-001, ERROR, §12.2) was only a line in the item analysis: never a review item, never blocking release | Each confident SCO-001 is a review item naming the item and the distractor, never the key letter. Release is refused ("computing…") until the flag-stability job has judged the current run, then until each item is decided: change the key to the distractor, accept both answers, void the item, or keep the key with a reason and a note (an audited `ReviewItemAcknowledged`). A key change is an audited key version; flags that disappear are withdrawn, ones that come back reopen; a later batch carries the decisions over. Low-confidence flags (SCO-002 `proposed_miskey`) never block | `omr-app/tests/flag_stability.rs::a_planted_mis_key_blocks_release_until_the_instructor_decides_it`, `…::a_mis_key_is_decided_by_changing_the_key_and_the_flag_is_gone`, `…::accepting_a_second_answer_or_voiding_decides_a_mis_key` |
| C2 | On the CS 522 key photos every row looked "erased and changed" (MRK-003): 48 of 58 rows highlighted, and the key summary hid why | The residue that makes a row "erased" must stand out from the page's own unmarked bubbles (95th percentile of their darkness, on photos), so a photo's uneven light no longer reads as erasures; the summary prints the reason beside each highlighted row. Real key photos: **0 of 58** rows highlighted (ARTI 503 scans 0 of 56) | `omr-recog` `decide::a_photo_residue_must_stand_out_from_the_pages_unmarked_bubbles`; gated `omr-cli/tests/real_key_read.rs` (fewer than 5 % highlighted) |
| C3 | A photographed sheet was a camera capture only if the test said so (the photo suites forced it on every other photo), and student photos were auto-accepted although the phone slice is not calibrated (§8.10) | A capture is a photo from its file (a colour JPEG below 200 ppi, a camera's EXIF tags) or its geometry (a corrected lattice, anisotropy); photo pages take the camera thresholds; every answer on a photographed student sheet goes to review (MRK-001 `uncalibrated_capture`) and the batch says so (CAP-003 WARN `phone_uncalibrated`). Photo suites now use the production classification: 200 photos, 200 decided as photos, 3 refused, 5,617 of 5,800 answers auto-accepted at recognizer level, **0 silent** | `omr-app/tests/iau_capture.rs::c3_phone_photos_of_students_sheets_go_to_review_until_the_phone_slice_is_calibrated`; `omr-imaging` `camera_exif_and_photo_resolution_mark_a_phone_capture`; `omr-recog/tests/photo_synth.rs`, `photo_adversarial.rs` |
| C4 | A student who bubbled the marker number and answered exactly like the key was taken as a second key sheet: nothing disagreed, so the student was silently not graded | Whenever a version is given by more than one key sheet and one was found in the stack, every one is named (file and page) in the hold, the key summary and its HTML, with the way to get a student graded (`--key-sheet FILE --key-sheet-marker none`) | `omr-app/tests/key_sheets.rs::a_marker_student_who_agrees_with_the_key_is_named_never_dropped_silently` |
| C5 | The key summary's row crops showed the bubbles only, so a sheet read one row off looked right under its question | Each crop starts 2.3 pitches left of option A and shows the question number printed beside the row; the summary tells the instructor to check it against the Q column | `key_sheets.rs::a_key_summary_row_crop_shows_its_own_printed_question_number` |
| C6 | The remedies of REG-003 and REG-004 lived in the desktop shell only; the CLI and review queue gave none | Every blocking registration code has its remedy in the core review queue: REG-003 rescan flat (a photo: the whole sheet, from straight above, evenly lit), REG-004 check the form is printed at 100 % and rescan, a row-check refusal rescan unfolded or decide from the paper sheet; never key answers from the refused page | `omr-app` `review::tests::every_blocking_registration_code_has_a_remedy` |
| C7 | A key sheet read from the desktop or by `omr key read` and then used lost its provenance (the key version named no sheet); own-form key sheets were untested | `ExamProject::add_key_sheets` stores the original, holds the run and publishes a key version whose source is the key sheet with its SHA-384, from Setup's *Read key from answer sheet…*, the New exam form's *Key sheets* field and `omr key replace --from-key-sheets`; own forms read key sheets under the 9-digit scheme | `omr-app/tests/key_sheets_ownform.rs` (2 tests); `omr-cli/tests/key_sheets.rs::key_sheets_added_to_a_graded_exam_are_stored_held_and_confirmed`; `shell-slint/tests/req_a11_real.rs::req_a11_real_backend_new_exam_reads_the_named_key_sheets`, `…::req_a11_real_backend_reads_a_key_from_answer_sheets_in_setup` |
| C8 | Flag stability used Fisher-z CIs below N = 1,000 where §12.3 asks for the bootstrap; it was seeded from the run ID, so the same answers through two key histories gave different labels; a share of 51–52 % at 1,000 resamples was "confident" | Below N = 1,000 every item gets a percentile CI of r_ir from the same 1,000 resamples (`item_analysis.csv`: *r_ir bootstrap CI*); the seed is the SHA-384 of the scored content; a flag is confident only when its share's Monte Carlo 95 % interval lies above 50 % and the rule holds at both CI ends. The one confident CS 522 flag (51–52 %) is now low confidence | `omr-analytics/tests/stability.rs::stability_is_seeded_from_the_content_and_deterministic`, `…::the_bootstrap_gives_every_item_a_percentile_ci_of_r_ir`; `stability::tests::a_share_near_half_is_low_confidence_until_monte_carlo_noise_is_excluded`; `flag_stability.rs::the_same_content_through_two_key_histories_gives_identical_flags` |
| C9 | Throughput had not been re-measured after the second round | Re-measured on the real PDFs against the acceptance-run binary, alternating on the same machine (§2.2, §2.3): pages per second within the machine's noise; a 30-sheet run 0.1–0.3 s slower than the acceptance-run code, well inside A3's 3 s | `omr bench` |
| C10 (low) | SCO-027 printed the key's letter counts, exam-confidential, in warnings that reach the CLI and logs | The warning says what pattern was found (a run, a skewed distribution) and its p-value, never the counts; the counts appear only in the key summary for the instructor | `omr-scoring/tests/a8_oracles.rs` |
| C11 (low) | README, crate READMEs and IMPLEMENTATION described the second round's state | Updated: key sheets, the mis-key decision, photos to review, the known limits | — |
| C12 (low) | Exports printed Rust enum names (`ProbableMiskey`, `Low`), had no flag order, and the SCO-023 note (small N: item statistics below N = 30, with bootstrap CIs and low-confidence badges) was missing from the summary | The §12.2 messages, `normal` / `low (proposed)` / `possible, insufficient data`, a *Flag order* column (rank, then stability), and the SCO-018 / SCO-023 notes in `summary.json` and `report.html` | `omr-report` `items::tests`; golden `items.csv`, `report.html` |

### 3.1.4 Version and key backstops (G-3)

Checks below the New-exam form, so a wrong default or a hurried click never scores a sheet
against a key the evidence does not support (synthetic IAU sheets; `omr-app/tests`). The
version rows apply to exams created since the backstops (definitions with
`hold_undeclared_versions`); older projects keep their routing (§7).

| Case | What happens now | Test |
|---|---|---|
| A sheet bubbles a version the exam does not declare (C on an A, B exam; B on a one-version exam) | Held: SCO-005 `undeclared_version` (ERROR, a Form item naming the letter), no version, out of the score run and the exports until a reviewer chooses its version; the answers suggest a version only by 10 nats. **Was:** given a provisional version by key likelihood, or silently scored as the one version | `version_routing.rs::every_version_set_routes_every_bubble_and_holds_undeclared_letters` (versions [A], [C], [A,B], [A,B,C,D] × bubbles A–D, blank, unclear), `…::a_reviewer_gives_a_held_sheet_its_version_and_it_is_scored`, `…::a_definition_saved_before_the_backstop_keeps_its_routing_and_bytes` |
| A blank version bubble when every version has the same key, points and mapping | Scored under the shared key, SCO-005 `identical_forms` (INFO, not queued); "version not declared" in the results' Form column. **Was:** SCO-005 `inferred` ERROR with margin 0 on every such sheet (CR-16) | `version_routing.rs::identical_versions_score_an_undeclared_sheet_under_the_shared_key_without_review` |
| A key file labelled for other versions than the exam declares: a single `C` column on an A, B exam, or an A–D key with `--forms A,B` | Refused, naming the versions and what to do; columns of undeclared versions are left out only when they are empty (a template's spare columns). A key that names no version (`key`/`answer` column, one column of answers, headerless `1,B`, a `*` row) is every version's. **Was:** the single C key was copied to A and B, and the C and D answers were dropped silently | `instructor_files.rs::a_key_labelled_with_one_version_is_never_copied_to_another` |
| Key answers beyond the exam's question count: rows of a long key file, columns of a wide one, lines of a single column, rows marked on a key sheet | Not part of the key, as before, but named: `KeyFile::ignored_questions` and a warning from the New-exam definition and `omr grade` / `omr key replace` ("questions 31–35 have an answer but the exam has 30 questions … set the question count to 35"); a key sheet's rows past the count are read once more at the sheet's full count and named in the key summary. **Was:** dropped silently, so a count typed too low lost questions without a word | `instructor_files.rs::key_rows_beyond_the_question_count_are_named_in_every_layout`, `key_sheets.rs::key_sheet_rows_beyond_the_question_count_are_named_in_the_summary` |
| A one-column key with an empty cell, as Excel or Sheets saves it (`B`, `C`, empty line, `A`, `D` with CRLF or LF; also a line of spaces or `""`) | The empty line is that question without an entry: the key reads B, C, –, A, D, the New-exam proposal shows the gap at question 3 (and asks), and an error after an empty line names its true line. Empty lines before the first answer and after the last are not questions. **Was:** the CSV reader skipped the empty line, so A became question 3's answer and every later answer moved up a question, with no gap shown and the count one too few | `instructor_files.rs::an_empty_cell_of_a_one_column_key_is_a_gap_never_skipped`, `exam_proposal.rs::an_empty_cell_of_a_one_column_key_is_asked_as_a_gap` |
| An own sheet whose spec prints `cross_cancels` or `fill_cancels_check`, defined from the desktop's New exam | The definition takes the spec's marking convention (a typed one must agree); a spec without one is refused naming the file and the field. The key-sheet rule (markers, detection in the stack) reaches the New-exam definition as it reaches `omr grade`. **Was:** the definition took `erase_only` and every such spec was refused at validation | `instructor_files.rs::an_own_sheets_cancel_mode_comes_from_its_spec_and_the_key_sheet_rule_from_the_inputs` |
| A key changed on an exam with held sheets | A held sheet's suggestion follows the new key, still only by 10 nats: none under identical keys, A again under the versions' own keys, none for sheets answering C's or D's key | `version_routing.rs::a_key_change_refreshes_a_held_sheets_suggestion_only_by_a_10_nat_margin` |
| Identical versions while key sheets in the stack wait for confirmation | A sheet without a version is inferred and reviewed (SCO-005 `inferred`), never scored under the shared key: the key sheets may still make the versions differ | `version_routing.rs::a_sheet_without_a_version_is_inferred_while_key_sheets_wait_for_confirmation` |
| A sheet scored under the shared key, in the parallel run with Remark | It declares no version: paired by its answers with a row of either version's workbook, and reported under that workbook's version | `remark_compare.rs::a_sheet_scored_under_the_shared_key_pairs_with_any_versions_row` |
| *Apply to group* on version (Form) items | Refused when an item's margin is under 10 nats or it bubbles an undeclared version; nothing changes. **Was:** a tied group (margin 0) was confirmed in one keystroke | `version_routing.rs::versions_are_confirmed_as_a_group_only_by_a_10_nat_margin` |

### 3.1.5 The New-exam proposal and its survey (G-3)

Before an exam is defined, `omr_app::survey` reads the scans once in memory (every printed row,
version and key sheet; counts only) and `omr_app::propose` proposes every value of the
definition with its source, asking when the evidence disagrees; `omr survey` prints it. Each
proposal that gives inputs is graded with them in the test (synthetic IAU sheets, `omr-sim` own
sheets, synthetic Remark workbooks; `omr-app/tests/exam_proposal.rs`).

| Case | What the proposal does | Test (`exam_proposal.rs::…`) |
|---|---|---|
| Counts: versions bubbled, rows answered, key sheets in the stack, student numbers that fit the scheme | Counted (24 students: A 8, B 8, C 6, blank 2; rows 1–40 on every sheet; 3 key sheets named by page); nothing written; no student number in the survey, the proposal or its JSON | `the_survey_counts_versions_rows_and_key_sheets_and_keeps_no_student_number` |
| Stray marks on row 77 (2 of 40 sheets), blank backs, bleed-through, a foreign page named by a student number | The extent stays 30 (high); blank pages counted apart; the foreign page named by file and page, masked | `stray_marks_far_below_the_questions_do_not_move_the_extent`, `pages_that_are_not_the_sheet_are_counted_and_named` |
| The same files in another order, at 1 or 4 threads | Identical survey (fingerprint included) and proposal | `the_survey_and_the_proposal_do_not_depend_on_input_order_or_threads` |
| One version and a one-column key; two or four versions named by the key | Versions A / A, B / A–D, the count from the key; grading: no version review, two inferred blanks, every sheet scored under its own key (hand count) | `p1_…`, `p2_…`, `p3_…` |
| Six sheets bubble C on an A, B key; one bubbles D | C asked ([choose another key] [hold the 6 sheets], and [read key version B as C] when B has no sheets); D a note; after the answer both held, never scored | `p4_p5_a_version_the_key_lacks_is_asked_and_a_stray_one_noted_then_held` |
| One answer column while A and B are bubbled on many sheets | Asked; [the same key for every version] gives A, B with the shared key: blank bubbles scored under it, no SCO-005 queued | `p6_one_answer_column_with_two_bubbled_versions_asks_then_shares_the_key` |
| The count: the key's last answered question (empty template rows ignored); typed lower (asked: use 40 / keep 30); typed higher or a gap in a version (asked: nothing is scored until filled); a question beyond the sheet; option E with 4 typed | As listed; typed values are never replaced | `p7_p9_the_count_and_the_gaps_come_from_the_key_file` |
| Numbered versions (`Version 1`), unlabelled key columns, headerless rows starting with their version | Asked, then read as the letters answered; labelled rows read with a note | `numbered_and_unlabelled_key_versions_are_asked_and_read_as_answered` |
| Students answer rows 31–40, the key covers 30; key sheets mark 35, the students 30; a stack with a short exam inside | Asked; the count from the key sheets with a note; asked ("may hold two exams") | `p8_…`, `p10_…`, `p14_…` |
| No key | Asked, never inferred by default; after [provisional key] the version with ≥ 10 sheets gets an inferred key and the 2-sheet version is held | `p11_no_key_is_asked_and_a_provisional_key_is_only_chosen` |
| A Remark import | Its versions and count; a version it lacks, a typed count or a key file beside it asked; its keys and points in the definition | `p12_a_remark_import_gives_the_versions_and_the_count` |
| Labels and typed values | Title from the exam-type bubble (5 of 6), course from the folder name, term from the last exam of the exams folder (within 120 days); typed ones stay, a contradicted one is asked | `p15_p16_labels_come_from_the_evidence_and_typed_values_stay` |
| A survey of other files (or a scan replaced since) | Ignored; the scans are to be surveyed again | `p17_a_survey_of_other_inputs_is_ignored` |
| Own sheets | Versions from the verified page codes; `cross_cancels` from the spec, graded with nothing typed; a spec without one asked | `s6_p13_own_sheets_are_surveyed_by_their_page_codes_with_the_specs_cancel_mode` |
| The setup | Each value with its source and evidence recorded with the exam (`ExamDefinition::provenance`, in the setup event and the snapshot), counts only | `the_setup_records_where_each_value_came_from` |
| Versions typed (or declared from the card) with a key file of one answer column | Asked whether it is every version's key, with or without a survey, also after the card's own [Declare A, B]; answered, both versions take it, sheets without a version are scored under it, and the card promises them no version review. **Was:** typed versions skipped the question, so version B's sheets were scored with A's key without a word, and the card noted a version review grading never made | `typed_versions_with_one_answer_column_ask_whether_it_is_every_versions_key`, `p6_typed_versions_with_one_answer_column_share_the_key_without_version_review` |
| Headerless key rows `A,…` and `C,…` of 20 cells, the count typed as 20 | Asked: the first cell is the version (19 questions) or the answer to question 1 (two keys of 20, their versions asked in order); under each answer the card's versions, count and gaps are the keys `define` reads, since the proposal gives `define` its reading (`Inputs::key_rows_labelled`). **Was:** the card showed versions A, C and a gap at question 20 while `define` read two unlabelled keys of 20 answers and graded at once | `headerless_rows_starting_with_a_letter_are_asked_when_the_count_reaches_the_row` |
| A key template `question,A,B,C,D` with C and D empty | [Use versions A, B] beside [Grade now; scores wait for the key]; graded with it, every sheet is scored. **Was:** the only grading choice gave an exam with no score run (SCO-003 for every C and D question) | `a_key_templates_empty_version_columns_can_be_left_out` |
| Versions typed A, B with a key `question,1,2` | Asked which letters 1 and 2 are ([Read 1, 2 as A, B]); the answer reads the columns as those versions. **Was:** a dead end: the answer was ignored and the only choice was another key file | `typed_versions_read_a_numbered_key_file_as_answered` |
| Versions typed A, B while 3 sheets bubble C | [Declare A, B, C] only when the key source has a key for C (else another key file, or hold the 3 sheets); declared, the C sheets are scored under C's key. **Was:** offered with a key file lacking C, leading to a refused definition | `declaring_a_bubbled_version_is_offered_only_when_the_key_has_it` |
| A make-up stack of 1 or 2 sheets, all bubbling B, with a one-column key | Version B, every sheet scored; 1 B beside a blank sheet stays a stray (version A, the B sheet held). **Was:** version A was declared and every sheet held | `a_make_up_stack_bubbling_one_letter_declares_it` |
| A key sheet in the stack beside a Remark import (or a key file); key sheets named beside an import | Noted: the key sheets are laid over the import's key and the run waits for their confirmation, as grading does; named key sheets asked ([Use the Remark import] / [Use the key sheets]). **Was:** no note beside an import, and the graded run waited unannounced | `key_sheets_beside_a_remark_import_or_a_key_file_are_noted_or_asked` |
| A one-column key with an empty line at question 3 | The gap asked at question 3, the count 5 (§3.1.4) | `an_empty_cell_of_a_one_column_key_is_asked_as_a_gap` |
| Rules no test covered before: a key file that cannot be read, a typed cancel mode the sheet does not print, a folder that holds an exam, no scans, a roster `define` refuses, a count or options out of range; rule 5 and C5 on key sheets, key sheets that cannot give a key, a stack mostly of another sheet, two exam-type bubbles; the extent's three conditions and a letter on under 5 % of the sheets; a survey made before key sheets or a roster were named; a blank-version key sheet on a one-version exam, a double mark counted | Asked or noted as each rule says; each test fails with its rule removed | `files_and_typed_values_the_exam_cannot_use_are_asked`, `key_sheets_and_pages_that_cannot_serve_the_exam_are_asked`, `p14_two_exam_types_in_one_stack_are_asked`, `the_extent_is_high_only_when_the_last_row_the_body_and_the_tail_all_say_so`, `a_letter_on_few_sheets_is_a_stray_and_weak_evidence_names_no_title`, `p17_…`, `a_key_sheet_without_a_version_serves_a_one_version_exam_and_double_marks_are_counted` |
| `omr survey` | Exit 2 while a question is open (each with its `--answer`), 0 once answered; `--json`; nothing written | `omr-cli/tests/survey.rs` |

Survey cost on these stacks, one test at a time (test profile, `opt-level = 1`, 200-dpi PNG
IAU sheets, Linux container): 10 pages in 1.30 s at 1 thread and 0.50 s at 4; 27 pages in
1.03 s at 4 threads; 40 pages in 2.58 s at 2 threads; 7–14 pages in 0.6–1.1 s at 2 threads.
Release figures are not measured yet; the survey is one more recognition pass than grading
alone.

### 3.1.6 Cancelling a run (G-3)

`omr_app::Cancel` is the operator's cancel of a new exam's grading, of scans added to a graded
exam, or of a survey: the run checks it before the project package is created, before each
page is decoded and between stages, never once the store stage has begun (synthetic IAU
sheets; `omr-app/tests/cancel.rs`). The desktop's *Cancel grading* is the shell's part of G-3.

| Case | What happens | Test (`cancel.rs::…`) |
|---|---|---|
| Cancelled before the run starts | `AppError::Cancelled` ("cancelled by the operator; nothing was saved", exit code 130); no package created, no page read | `a_run_cancelled_before_it_starts_creates_no_package` |
| Cancelled while the first page is read | No further page is decoded and no stage runs; the package (its originals already spooled) and the exam folder the run made are removed; the same folder then grades from scratch. **Was:** an empty exam folder stayed behind after a failed run | `a_cancel_during_the_page_stage_removes_the_package_and_its_new_folder` |
| Cancelled at the batch, identity or scoring stage | Stops before the next stage; nothing stored | `a_cancel_between_stages_stops_before_the_next_one` |
| Cancelled once the store stage has begun | Too late: the exam is written whole, opens and passes the audit checks | `a_cancel_once_the_store_stage_has_begun_is_too_late` |
| Scans added to a graded exam, cancelled while the first new page is read | The exam's results stay as they were; the run's spool stays (its scan batch left open, the new originals and page rows); adding the same files again adds their 2 pages and 2 booklets, and the exam reopens and passes the audit checks. **Was:** adding them again failed for good (`UNIQUE constraint failed: page.page_id`), opening another batch each time | `a_cancelled_append_keeps_its_spool_and_the_same_scans_add_again` |
| A survey cancelled while it reads | Stops with `Cancelled`; a survey with a fresh token runs to its end | `a_survey_honours_the_cancel` |

### 3.1.7 A roster after grading (G-3)

`ExamProject::set_roster` gives a graded exam a roster (or replaces it): the rows are imported
as `--roster` imports them, and every booklet is identified again over the stored originals,
as an appended batch is graded again (synthetic IAU sheets and rosters;
`omr-app/tests/roster_after_grading.rs`). The desktop's *Roster* step is the shell's part of
G-3.

| Case | What happens | Test (`roster_after_grading.rs::…`) |
|---|---|---|
| 8 sheets graded without a roster and reviewed, then a roster of 9 (7 of the sheets' students, 2 who handed in nothing) | 7 booklets matched, 1 not on the roster (an identity item), 2 absent; booklets, items and `results.csv`, `booklets.json`, `item_analysis.csv` byte-identical to grading with the roster from the start; a decided mark and a deferral carry over. Audited: the roster snapshot and `RosterReplaced` (row count and hashes, no student data) in one decision with the identity run that names the snapshot | `a_roster_after_grading_matches_the_students_and_keeps_the_decisions` |
| A roster that lists a student twice; a roster whose run is cancelled, or cannot read the stored originals | Refused or failed with nothing changed: no roster, the same booklets and items, no event; the next run (a file added again) records no roster, and the next roster is the only one recorded | `a_refused_roster_changes_nothing` |
| Released without a roster, then the roster given | The release is superseded (`ReleaseSuperseded`, "roster replaced"): no Blackboard upload until released again; then the upload lists every student who handed in a sheet by roster ID | `the_blackboard_upload_appears_after_the_roster_with_a_provided_key_and_release` |
| Reopened after the roster; a later batch; a second roster | Opens (also read-only) and passes the audit checks; the later batch is identified against the roster in force and its identity run names it; the second snapshot's parent is the first. The exam row keeps the identity mode it was created with (`docs/IMPLEMENTATION.md`) | `an_exam_given_a_roster_reopens_clean_and_keeps_it_for_later_runs` |

### 3.2 Security and code review: what was fixed

A security review and a code review of the MVP raised 26 findings above "low" (SEC-01…SEC-11,
CR-01…CR-15) and 12 low ones. Each was reproduced before it was fixed, and each code fix has a
regression test; all of them passed in the acceptance run. One did not reproduce (SEC-03: the
multi-page TIFF fix of §3.1 had already landed).

| Finding | What was wrong | What happens now | Tests |
|---|---|---|---|
| SEC-01, CR-01 (critical) | Grades were rebuilt from `cache/session.json`, which nothing protected: editing it changed released grades and the Blackboard upload with exit 0 and no audit trace | Every save of the snapshot is bound to the audit chain (`SnapshotSaved`: SHA-384 of every part); on open it must match, and the decisions in it must equal the store's decision log, else the project is refused with SYS-011 and what to do. Eight tampering cases (edits, truncation, re-hashed rewrites, edited state tables) are all detected | `omr-app/tests/audit_integrity.rs` (8 tests); `omr-store/tests/decisions.rs::decision_tables_edited_behind_the_chain_are_refused_on_open` |
| SEC-02 | No signed checkpoint: truncating or re-chaining the log went unnoticed; the export manifest was unsigned | Every change ends with a checkpoint signed by a per-device ECDSA P-384 key and kept outside the project; an older copy of the project is refused; `manifest.json` carries the signature | `an_older_copy_of_the_project_is_refused_against_the_device_checkpoint`, `the_export_manifest_is_signed_with_the_device_key` |
| SEC-03 | A multi-page TIFF graded as its first page | **Not reproduced**: already fixed (§3.1); a 3-page LZW TIFF gives 3 booklets | `every_page_of_a_multi_page_tiff_is_graded_and_a_damaged_rest_is_cap001` |
| SEC-04, SEC-06, CR-12 | Every `--example-spec` had the same exam ID, so another exam's sheets verified; the page-code key was written world-readable beside the print PDF and overwritten silently; a 9-digit ID grid was read under the 10-digit IAU scheme | A fresh exam ID per spec; the key goes owner-only to the private key folder, never overwritten without `--new-key`; a second project of the same exam is refused; the ID scheme follows the printed grid | `omr-cli/tests/cli.rs::own_form_first_use_with_default_flags` |
| SEC-05 | Projects and exports could be written into OneDrive, Dropbox or network folders | Refused with SYS-015 for projects, exports and crops | `omr-app/tests/privacy.rs::sync_folders_are_refused_for_projects_and_exports` |
| SEC-07 | Every file world-readable | Project folders 0700, files and exports 0600 | `the_package_and_every_export_are_private_to_the_user` |
| SEC-08 | Reviewer notes kept in plain text in the snapshot, with an unsalted hash in the log | Notes are stored by salted reference in an erasable table; the log keeps only the reference | `reviewer_notes_are_salted_references_and_can_be_erased` |
| SEC-09 | `review list --crops` wrote identity crops anywhere, unaudited; student numbers printed in full | Crops are an audited, owner-only export with a signed manifest; student numbers are masked to their last 3 digits unless `--reveal`, which is logged | `review_crops_are_an_audited_restricted_export`, `student_numbers_are_masked_outside_the_exports` |
| SEC-10 | The snapshot was written without fsync; a damaged one panicked or gave no remedy | Staged, synced and renamed after its event; an interrupted save is completed on the next open; a damaged one is SYS-010 with what to do | `an_interrupted_save_is_completed_on_the_next_open`, `a_damaged_or_missing_snapshot_is_sys010_with_remediation_never_a_panic` |
| SEC-11 | Files were read whole before any size check; a small file could decode to a huge image | Files over 2 GB are refused unread; an image above 100 megapixels is refused from its header, before decoding. Streaming and a per-run budget remain deferred (§7) | `omr-imaging` `rejects_garbage_and_limits` |
| CR-02 | A 4-option exam on the 5-option IAU sheet aborted the whole run when a student filled E | The mark is queued as MRK-017 `option_not_offered` and that question stays pending | `omr-app/tests/common_cases.rs::c11_four_option_exam_on_five_option_sheet` |
| CR-03 | Roster students with no sheet appeared nowhere | Listed as absent in the results and the summary | `c12_absent_roster_students_are_listed_absent` |
| CR-04 | The desktop app opened invented demo data and could not create an exam, load a key or release | Opens your exams folder; demo data only with `--demo`, marked on every screen; New exam, Replace key and Release results work from the app | `shell-slint/tests/req_a11_real.rs` (`…creates_an_exam_from_the_new_exam_form`, `…replaces_the_key_from_setup_and_names_a_tampered_project`, the review flow), `req_a11.rs::req_a11_demo_data_is_marked_on_every_screen` |
| CR-05 | The README said "design phase" and explained nothing | A user guide: build, grade with or without a key, review, release, the desktop app | `README.md` |
| CR-06, CR-08, CR-14, CR-15 | Evidence gaps: open privacy controls not disclosed, A11 not run, A4 measured without the save, real-data scoring shown beside "Pass" rows | This document: §2.4, §2.11, §7, and the real-data row of §1 | — |
| CR-07 | No release step; the Blackboard upload was written for unreleased results | `omr release` and *Release results…*: refused while anything blocks release; the upload exists only for released results and is removed when a later change supersedes the release | `project_actions.rs::c8_blackboard_grade_center_upload_holds_only_what_it_needs`; `omr-cli/tests/real_world.rs` |
| CR-09 | Launching the app without a display printed a raw error | The error says what to do (desktop session, `--screenshot`, or the CLI); the Windows release build, which has no console, shows it in a window of its own, or in the system's message box when no window of the app can open. A real window, this launch error on Windows and the native file dialogs remain untested (§7) | — |
| CR-10 | The section and exam type read from the IAU sheet were discarded | Carried into the exports; a sheet whose section disagrees with the roster is IDN-004 `cross_section` (INFO) | `c13_sheet_section_is_carried_and_checked_against_the_roster` |
| CR-11 | No teacher-filled key sheet | Built after the acceptance run: key sheets named (`--key-sheet`, `omr key read`) or found in the stack (student number all 0 or all 9; on a two-version exam a key sheet must also show its version, else it is refused with what to do), never graded as students, the run held (SCO-003 BLOCKER) until the instructor confirms the rendered key summary; rows that are not one confident mark block the confirmation, and erased-and-changed rows and rows a key file contradicts are highlighted; a key sheet showing no marker is refused, never read; a correction after confirmation survives a later rescan of the key sheet (§3.1.1, §4.1) | `omr-app/tests/key_sheets.rs` (8 tests), `omr-cli/tests/key_sheets.rs`; real: `real_iau_scored.rs`, `omr-cli/tests/real_key_read.rs`, `omr-recog/tests/real_key_sheets.rs` |
| CR-13 | A failed grade left a half-made project that every command then refused differently | The folder is removed when grading fails; an unfinished one is named with what to do | `c14_an_unfinished_grading_run_is_named_with_its_remedy` |
| SEC-12, CR-20 (low) | A chain failure printed no code or remedy; the app's read-only fallback blamed "another program"; some input errors had no remedy | SYS-011 with the remedy everywhere; read-only only when another process holds the store; input errors say what to do; next commands quote paths with spaces; `acceptance.json` records the digest of the code it measured (§6) | `req_a11_real_backend_replaces_the_key_from_setup_and_names_a_tampered_project`, `ingest::tests::input_refusals_say_what_to_do`, `omr` `next_commands_quote_paths_the_shell_would_split` |
| SEC-16 (low) | Roster warnings printed full student numbers | Masked to the last 3 digits, as in `review list` | `student_numbers_are_masked_outside_the_exports` (the masking they share) |
| CR-17 (low) | A not-on-roster sheet was offered the nearest roster student even when that student already held a sheet, a dead end | That student stays a listed candidate, never the one-key proposal | `iau_sheets.rs::c10_a_roster_neighbour_who_already_holds_a_sheet_is_never_proposed` |
| CR-18 (low, first part) | INFO notes such as MRK-016 counted as warnings that "need a look" | Counted at their own severity (the Scan card still counts superseded rescans with byte-identical copies) | `synthetic_e2e.rs::req_a10_corrupt_inputs_inferred_key_and_view_models` |

## 4. Real data: 143 IAU answer sheets

The owner supplied 143 real, student-filled IAU CCSIT answer sheets in five scanner PDFs:
CS 522 (two batches, 29 questions, versions A and B) and ARTI 503 (sections AI1 and MS1, 28
questions, versions A and B), scanned on Canon MFPs as 600-dpi bitonal (CCITT G4) images. They
are **restricted personal data** under the PDPL: they stay on the evaluation machine, nothing
from them is committed, and only the aggregates below are reported.

**Reference labels.** Each page was read independently by two readers and by the Python
prototype. The three readings agreed on 4,091 of 4,096 answers; the 5 answer and 8 header-field
disagreements were adjudicated on 600-dpi zooms. The readers and the adjudicator were AI
agents, so the labels are **agent-adjudicated, not certification-grade**: the spec never
certifies an error bound on agent labels alone (§23.1).

**Answer keys were inferred in this run.** The answer keys had not been supplied when it was
made, so these runs use `--infer-key`: each version's key is the most common confident answer
per question. Every output of such a run is marked INFERRED and is not releasable until the real
key replaces the inferred one. Accuracy
below is measured against the reference labels (what the student marked), which does not depend
on the key. Scoring with the real keys, supplied later, is §4.1.

Tests (env-gated, skipped without the data): `omr-app/tests/real_iau_e2e.rs::req_a1_real_iau_full_pipeline`
(the whole application as two exams), `omr-recog/tests/real_iau.rs::req_a1_real_iau_sheets_zero_silent_errors`
(recognition in three baseline modes) and `omr-recog/tests/ownform_real_iau.rs` (the own-form
path refuses all 143 legacy pages with REG-001 rather than mis-reading them).

| Measure | Result |
|---|---|
| Pages registered and read | 143 / 143 (registration residual median 0.44 px, worst 1.24 px at 200 dpi) |
| Questions | 4,096 |
| Auto-accepted | 4,089 |
| Silent errors | **0** (bound 3/4,096 = 7.3 × 10⁻⁴) |
| Review rate | **7 / 4,096 = 0.171 %** |
| Student numbers, whole app (rosterless, IAU scheme) | 138 exact, 5 sent to review, **0 wrong** |
| Student numbers, recognition only | 140 exact, 3 flagged, 0 wrong |
| Section (recognition) | 142 exact, 1 flagged, 0 wrong |
| Version (recognition) | 142 exact, 1 flagged, 0 wrong |
| Version / form, whole app | 140 declared and correct, 0 declared wrong; 3 inferred from the answers: 1 faint bubble, inferred as the version the reference shows; 2 blank bubbles, which the reference cannot check, both queued for confirmation (SCO-005) |
| Exam type (recognition) | 141 exact, 2 flagged, 0 wrong |
| Pages taken for blank backs | 0; every page's upright-vs-upside-down margin ≥ 0.511 (gate 0.20) |
| Throughput (`omr bench`, 4 threads) | **73.2 pages/s** recognition, 67.8 pages/s full pipeline (19.4 and 36.9 pages/s at 1 and 2 threads; 76–89 at 4 threads in earlier, quieter runs; 61–64 after the third round, beside 59–69 for this run's binary on the same busier machine, §2.2) |
| Whole app, 143 pages, store and exports | 2.45 s (58 pages/s) |
| Rescans, refused or held pages | 0: no real page was superseded as a rescan, refused for its resolution or held as a foreign form; each 600-dpi page carries CAP-002 INFO (read from the 200-dpi working copy) |

The whole app sends two more student numbers to review than the recognition-only count
(3 `id_ambiguous`, 2 `id_blank`, from the rosterless IAU identity rules); none was wrong.

**Rerun after the second adversarial round** (photo verification, row and column checks,
ring visibility; `real_data.second_round` in `acceptance.json`). Recognition: the fingerprints of
every bubble measurement and of every decision in all three baseline modes are **identical** to
the engine measured above; 0 pages fail the row checks (the worst row's evidence of another
row's content is 0.22 against the 0.35 threshold; the smallest own-row correlation 0.14); the
smallest shift-verification margin is 0.334 (threshold 0.15). Whole app: the same counts (0
silent errors in 4,089, review 0.171 %, student numbers 138 / 5 / 0, forms 140 + 3 inferred,
0 wrong); 143 pages with the store and every export (flag stability included) in 2.8 s
(51 pages/s) at a load average of 2.4–3.9, against 2.45 s in the acceptance run (6.1 s in an
earlier rerun while another agent's desktop tests held the load near 7).

**Rerun after the third round** (§3.1.2; `real_data.third_round`). Recognition: the fingerprints of
every measurement and of every decision in all three baseline modes are again identical. Whole
app: the same counts (0 silent errors in 4,089, review 0.171 %, student numbers 138 / 5 / 0,
forms 140 + 3 inferred, 0 wrong); 0 pages taken for photos; 143 pages with the store and every
export, flag stability with the per-item bootstrap CIs included, in 3.16 s (45 pages/s) at a
load average near 2.

**What this does and does not show.** It shows that on this university's real forms, scanners
and students the software made no silent error in 4,089 accepted answers while asking a person
about 7. It does not certify a rate: 4,096 questions bound SER at 7.3 × 10⁻⁴, far above the
contract's 10⁻⁵, and the labels are agent-adjudicated. The sheets come from two courses of one
college.

### 4.1 Scoring with the instructors' keys and key sheets

The owner later supplied the instructors' key sheets: for ARTI 503 two 600-dpi bitonal scans
(versions A and B, student number bubbled all 0), for CS 522 two phone photos in one PDF
(version A all 0, version B all 9). The verified keys (`question,A,B,points`) were read from them
by eye and, for ARTI 503, by the engine too; both agreed on all 56 answers. The keys are
exam-confidential: nothing below shows a key letter, and the class's own statistics (mean, pass
rate, KR-20) stay in the evaluation report outside the repository.

Tests (env-gated, also `OMR_IAU_KEYS`): `omr-app/tests/real_iau_scored.rs::req_a8_real_iau_exams_score_with_the_real_keys`
grades both exams with the key file, again with the key sheets named (`--key-sheet`) and again
with the key sheets inside the students' stack, confirming them as the instructor would;
`omr-cli/tests/real_key_read.rs` runs `omr key read` on the three key-sheet PDFs;
`omr-recog/tests/real_key_sheets.rs` reads the key sheets with the recognizer alone (also forced
onto the camera path). All were rerun in release by the second adversarial verification
(§3.1.1) and again by the third (§3.1.2), after their fixes, with the results below (the third
round's where they differ).

| Measure | Result |
|---|---|
| Booklets | 143 (CS 522: 92, ARTI 503: 51) |
| Scored booklets whose raw score equals the independent score (reference answers × key, computed in the test) | **137 of 137** |
| Provisional booklets | 6, each differing from the independent score only by its questions still in review (7 in all); a pending question is never credited |
| Auto-accepted answers differing from the reference | **0 of 4,089** |
| Forms wrong where the reference shows a version | 0 of 141 (2 reference versions blank; their inferred forms match the file they came from) |
| `omr key read`, ARTI 503 (600-dpi scans) | 56 of 56 answers equal to the verified key, none to decide, 0 rows highlighted |
| `omr key read`, CS 522 (phone photos) | 58 of 58 equal, none to decide, 0 rows highlighted for checking (48 of 58 before C2, §3.1.2), after the photo registration fix (F1). The previous engine registered both photos one bubble row too high and auto-accepted all 58 answers, 49 of them wrong, reading the student numbers wrong too (reproduced by rebuilding it); the app would have refused those sheets anyway (a key sheet whose grid shows no marker is never read as a key, proven on a synthetic sheet reading `1111111111`), but a student's photographed sheet had no such guard. Now every alignment is verified against the blank form's non-periodic content before anything is read (§3.1.1) |
| Decision files, key sheets named vs key file | **7 of 7 identical** for each exam (`results.csv`, `item_analysis.csv`, `test_stats.csv`, `tp154_a1.csv`, `booklets.json`, `review_queue.json`, `key.csv`) |
| Decision files, key sheets in the students' stack vs key file | both key sheets of each exam found by the rule; 7 of 7 identical for each exam |
| Mis-key flags with flag stability (§12.2, F2, C8), as exported | CS 522: 10 rank 1–4 flags, **all low confidence**, including its 4 probable mis-keys (rule 1). The one that held at both CI ends in the second round fires in 51–52 % of 1,000 bootstrap resamples, inside the Monte Carlo interval around 50 %, so it is now *(proposed)* (SCO-002), not a release blocker. ARTI 503: 8 flags, all low confidence or insufficient data, including all 7 probable mis-keys (the one-outlier items F2 reported). **No mis-key review item is queued on either exam**; the same flags whether the key comes from the file, the key sheets named or the key sheets in the stack. Before F2 every flag was exported as confident |
| Bootstrap CI of r_ir (§12.3, C8) | Every item whose r_ir is defined in ≥ 90 % of the resamples has a percentile CI: 52 of 58 items (CS 522) and 44 of 56 (ARTI 503); the others are left blank (nearly everyone answered them alike, so r_ir is often undefined) |
| Photographed pages among the students' sheets (C3) | 0 of 143 (every page a 600-dpi bitonal scan), so no answer was sent to review for an uncalibrated capture |

**What this does and does not show.** The app's arithmetic on the real exams equals an
independent count on every fully scored booklet, and the key read from the instructors' own
sheets changes nothing. It rests on the agent-adjudicated reference labels (§4) and on keys
verified by agents (by eye, and for ARTI 503 also by the engine); the instructors have not yet
confirmed them in the app themselves.

## 5. Remark Office OMR: import and parallel run

The owner supplied Remark Office OMR's graded Excel exports of both exams (per section and
version). Two tools read them, and both are tested on synthetic workbooks laid out as Remark's
export (invented IDs and names):
- **`omr import remark`** (`crates/omr-app/src/remark.rs`; spec §3.5 item 3, PAR-09) turns the
  exports into a key file with exact points per version, the learning objectives as weight-1
  CLO mappings "imported, needs review", and a version map. Every total Remark reported, and
  every learning-objective subtotal, is reproduced exactly before anything is written;
  otherwise the import stops and names the rows, the questions or the files. Tests:
  `omr-app/tests/remark_import.rs`, `omr-scoring` `points::tests`, `omr-cli/tests/remark.rs`.
- **`omr compare remark`** (`crates/omr-app/src/parallel.rs`; spec §23.7) sets a graded exam
  beside the exports: per question, per student with the exam's own totals (its score run)
  beside Remark's, the points that differ, identity differences, the learning objectives'
  subtotals, and the key sheets Remark graded as students. With `--queue` (desktop: *Queue for
  review*) every settled difference from an auto-accepted answer is a review item that holds
  release, and each decision is recorded as a field label. Its report carries the Restricted
  banner in every file, the licence-review (Q-08) status, a signed manifest and an
  `ExportCreated` event. Tests: `omr-app/tests/remark_compare.rs`, `omr-cli/tests/remark.rs`.

A **parallel run** on the two real exams was performed internally with these tools (the
env-gated `crates/omr-app/tests/real_iau_remark.rs`, `OMR_REMARK_DIR`): a development check by
the agents on data the owner supplied, not the parallel run of a pilot. The licence review the
spec requires for parallel runs and benchmarking (Q-08: the university's legal affairs, due
2026-12-01) is **still open**, and §23.7 asks for the comparison to run in the enclave or on the
exam control unit's machines with agents receiving counts. Its figures are held inside the
university and are **not in this repository**: the committed test asserts only what holds for
any exam, and what the run found is checked against a restricted expectations file kept with
the other restricted data (`OMR_REMARK_EXPECT`); `omr-archtest` fails the build on a
hard-coded count or identity-difference kind in a committed Remark test. Remark's binary files
(`.rmx`, `.aky`) were never opened. Agreement between the two systems is never counted as a
label (shared errors are invisible, spec §23.1).

### 5.1 Verification of the import and the parallel run

An adversarial verification reran both tools on the real exports (aggregates only, kept inside
the university), tried to break them, and fixed ten defects, each with a regression test that
fails on the code before it:

| What was wrong | What happens now | Test |
|---|---|---|
| (high) The exact solver eliminated in 128-bit fractions, whose size grows like the minors of the answer matrix: it overflowed from about 60 questions, so an export without learning objectives of a 60–100-question exam could not be imported, and the refusal named no remedy | Elimination modulo the prime 2⁶¹ − 1 (exact, no growth); each question's points are rebuilt as the fraction they reduce from and verified exactly against every total; a refusal is confirmed modulo a second prime and names what to do. A 100-question, 300-student system solves in under a second, and a hand-edited total among its rows is named | `omr-scoring` `points::tests::a_hundred_question_exam_is_solved_exactly_without_overflow`; `remark_import.rs::a_hundred_question_export_is_solved_from_the_totals` |
| (high) The first row whose student-ID cell had no digit was taken for the footer, and every row below it was skipped: a student whose ID Remark wrote as a word (`ERROR`, `BLANK`, an Excel `#N/A`) silently removed the rest of the class | A row is a footer only with statistics, not answers, in its answer columns (or a known label, or no total), each row judged on its own; a second `Answer Key` row (several tests in one report) is refused | `remark_import.rs::a_student_row_whose_id_remark_could_not_read_does_not_end_the_rows` |
| (high) Worksheets were read as dense grids: one value at the far corner (`XFD1048576`) of an 8 KB workbook made the reader request 549 GB and the process aborted, the desktop app with it; roster workbooks too | Cells are streamed and kept sparse (`omr-app` `xlsx.rs`); a value beyond 100,000 rows or 2,048 columns is refused naming its cell; a workbook whose parts declare more than 512 MB decompressed is refused before it is opened | `remark_import.rs::a_stray_value_at_the_far_corner_is_refused_not_an_abort`, `…::a_workbook_whose_parts_inflate_beyond_any_export_is_refused` |
| An ID match was taken whatever the answers: a booklet carrying another section's student's number was paired with that student's row, which put that section in scope and listed all its students as rows with no booklet | An ID match must agree as two readings of one sheet do (half the answers, and half of those where either departs from the key); otherwise both sides go on to answer pairing, and the ID match is paired last and reported ("same student ID, answers of another sheet") | `remark_compare.rs::a_booklet_with_another_students_number_is_paired_with_its_own_row` |
| Workbooks of another test of the same students (same question count) were compared booklet by booklet | When nothing pairs as a reading of the same sheet, the workbooks are refused | `remark_compare.rs::workbooks_of_another_test_of_the_same_students_are_refused` |
| A learning-objective header naming a question the export does not have (`Q2 (7-40)` on a 10-question test) was dropped silently, with its CLO | Refused: the two reports are not of one test. Questions in no objective, or in several, are named in the summary | `remark_import.rs::learning_objectives_of_another_test_or_with_gaps_are_named` |
| A student ID Excel had turned into `2.28E+09` was kept as an ID and, paired by answers, reported as one Remark could not read | The import says how many IDs lost digits and how to export them again; the comparison says the ID lost digits in Excel | `remark_import.rs::an_id_excel_turned_into_scientific_notation_is_said` |
| A workbook given twice counted every student twice | Refused naming both files | `remark_import.rs::a_workbook_given_twice_is_refused` |
| Confirming key sheets shared one item set across versions whenever their answers agreed, so a version with its own points (`form_points`, as the import writes them) was scored with version A's points | The confirmation applies the key file's rule: the same answers, the same points and no version-specific CLO row | `key_sheets.rs::confirmed_key_sheets_keep_each_versions_points` |
| With learning objectives imported per version, a cell of the Setup grid (a printed question and a CLO) stands for several rows; editing its weight or note changed the first matching row only, one version's, silently | Refused with the way to change them (the `form` column of `clo.csv`, then grade again with it) | `remark_compare.rs::a_mapping_cell_that_stands_for_several_versions_rows_is_not_edited_silently` |

### 5.2 Fifth round: the exam's own grades, the hold on release, points and CLOs

A fifth review raised eight findings above "low" (F1–F8, F12) and five low ones (F9–F11, F13).
Each was reproduced first (F1 and F4 also on the reviewer's own reproductions); each code fix
has a
regression test that fails on the code before it, and the gated real-data tests were rerun in
release on the fixed code (pass or fail only here).

| Finding | What was wrong | What happens now | Tests |
|---|---|---|---|
| F1 (high) | The comparison recomputed "our" total from the answers with Remark's imported points and its own single-answer rule: an exam graded at 1 point per question read as "every settled total equal to Remark's", and the report never said that the exam's points differ | The totals are the exam's score run (its points, voids and policy: the figures it releases), compared with Remark's total **and total possible**; the questions whose points differ and each version whose maximum differs are listed and warned about (CLI, HTML, desktop tile, `summary.json`); the recomputation with Remark's key and points is kept as a labelled "answers only" check of the readings | `remark_compare.rs::the_totals_are_the_exams_own_grades_not_a_recomputation_with_remarks_points` |
| F2 (high) | §23.7: every difference from an auto-accepted answer is adjudicated before release; the comparison held nothing, recorded nothing, while the HTML, the desktop and the CLI said the opposite | `queue_parallel_differences` (`omr compare remark --queue`, desktop *Queue for review*) makes each settled difference from an auto-accepted answer a Marks-lane MRK-001 `parallel_run_disagreement` (ERROR) item with both readings and its bubble row: the answer waits for review, the total is provisional and release is held; accepting keeps the exam's reading, another answer needs a reason; each decision is a `FieldLabelRecorded` event (engine loss, incumbent loss); the comparison is `ParallelRunCompared`; open items survive a later batch; refused on released results. Every text now says whether the report holds release | `remark_compare.rs::settled_differences_queued_for_review_hold_release_and_become_field_labels`, `…::a_queued_parallel_difference_survives_a_later_batch`; `shell-slint/tests/req_a11_remark.rs` |
| F3 (high) | The committed gated test hard-coded what the parallel run found on the real exams, which with `acceptance.json` recording it as passed disclosed head-to-head facts (Q-08) | The committed test asserts only invariants that hold for any exam; what the run found is checked against `OMR_REMARK_EXPECT`, restricted and outside the repository; its counts go only to the private report folder; a fitness test rejects hard-coded counts and identity-difference kinds in committed Remark tests | `real_iau_remark.rs`; `omr-archtest/tests/rules.rs::head_to_head_figures_of_the_real_exams_stay_out_of_the_repository` |
| F4 | A learning objective's points were split equally over its questions and only checked against the totals: when two of its questions were answered right by exactly the same students any split fits, and the guess was written as exact | The equal split is taken only when the totals and the group points allow no other; otherwise the import is refused naming the group, its questions and the equal split as a hint (`--points`, or the desktop's *Use the suggested points*). The real ARTI 503 exports still import (their points are determined) | `remark_import.rs::a_learning_objective_split_the_totals_cannot_pin_down_is_refused_not_guessed` |
| F5 | Imported learning objectives kept TP-154 in draft with no audited way to approve or replace them; versions whose objectives differ, and the domain K default, went unsaid | `omr clo approve` / `omr clo replace` and Setup's *Approve imported mappings* / *Replace CLOs*: an audited `MappingApproved` (the approver named) or `MappingReplaced`, the outcomes recomputed from the same scores; the import notes each objective's points and questions per version, warns when they differ, and says every CLO is in domain K | `remark_compare.rs::imported_learning_objectives_are_approved_or_replaced_without_grading_again`; `req_a11_remark.rs` |
| F6 | `A+C` and `A&C` were read as "either answer"; the spec's Remark syntax means "all of them" | AND keys (`+`, `&`, `and`) are refused in key files and Remark's Answer Key row with what to write instead; not scored yet | `exam` `tests::key_entries_parse`, `instructor_files` `c5_key_files_in_every_common_layout_read_the_same_key` |
| F7 | IMPLEMENTATION listed the Remark migration as built while most of §3.5 items 3, 5 and 6 were not | The narrowed parts are listed as deferred with their spec sections; the learning objectives' subtotals (CLO results, §3.5 item 6) are now compared | `remark_compare.rs::the_parallel_run_reports_agreements_differences_and_identities` |
| F8 | The migration's limits were not in the owner-facing docs | README §2, §5, §8 and §7 below list them with their workarounds; the Setup grid labels a cell that stands for one version's rows; the desktop's *Replace key…* reads `points_A`/`points_B`; a `.csv` in the desktop's Compare field gives handwritten IDs | `remark_compare.rs::a_mapping_cell_that_stands_for_several_versions_rows_is_not_edited_silently` |
| F9 (low) | `scripts/acceptance.sh` ran neither gated scoring nor Remark test, and a rerun would have dropped the later rounds from `acceptance.json`; the "100-question" test used 70 | Both gated tests run when their data is given (pass or fail recorded, nothing else); the report carries the recorded rounds forward; the test uses 100 questions | `remark_import.rs::a_hundred_question_export_is_solved_from_the_totals` |
| F10 (low) | A perfect score with no booklet (another section's student) was called a key sheet and removed from the "without key sheets" statistics | Only an ID of one repeated digit is a key sheet; such a row is a *possible key sheet or perfect score*, with the statistics both with and without it | `remark_compare.rs::a_perfect_score_without_a_booklet_is_a_possible_key_sheet_never_counted_as_one` |
| F11 (low) | Nothing said the Q-08 review is open or let a decision stop a comparison | README and §5 say so; the CLI and the report state the licence review as recorded in `licence_review.json` (private key folder), and a recorded refusal stops the comparison | `remark_compare.rs::the_licence_review_is_stated_and_a_refusal_stops_the_comparison` |
| F12 | The figures prepared for the owner, outside the repository, were out of date | Their scratch tool was fixed and the figures re-derived from the gated run; they stay outside the repository | — (scratch only) |
| F13 (low) | `omr import remark` wrote the exam-confidential key with no manifest or label | A signed `manifest.json` lists every file's and every source workbook's SHA-384 under the `restricted` label | `omr-cli/tests/remark.rs` |

Also checked, without a defect: a workbook re-saved by LibreOffice, or rewritten in Strict Open
XML (as Excel's "Strict Open XML Spreadsheet" saves it), reads like the original; mismatched
question counts, keys or learning objectives that differ between the workbooks of a version,
an empty answer cell, a total no points reproduce, IDs typed as numbers (lost leading zeros),
duplicate IDs across sections and a full-marks row under a real-looking ID behave as described
in `crates/omr-app/README.md`. Every file of the report carries the Restricted banner (the
manifest its classification), no student name is read or written, the CLI prints counts and
masked IDs only, and a scan of every changed repository file finds no student ID, name or key
from the real exports.

## 6. Measurement conditions

| Item | Value |
|---|---|
| Machine | Shared container, 4 vCPU (Intel Xeon @ 2.10 GHz), Linux |
| Build | Rust release profile (thin LTO), `scripts/acceptance.sh` |
| Load | Load average 2.29 at the start and 3.01 at the end; 3.4–3.9 during the timing rows (A2–A4), mostly from the acceptance run's own preceding tests (no other build was running) |
| Code | Commit `df5daee` plus the uncommitted work of the adversarial verification (§3.1) and of the security and code review fixes (§3.2). `acceptance.json` records the SHA-256 of the code that was built (`git.measured.code_sha256`); a commit holds that code exactly when `git ls-tree -r <commit> -- crates Cargo.toml Cargo.lock \| grep -v '\.md$' \| sha256sum` prints the same digest |
| Date | 2026-10-17 |

Timings on this machine are therefore conservative. The spec's throughput targets are set for
reference laptops (§21.1), which were not available.

**Reproduce.** `scripts/acceptance.sh` runs the A1–A11 rows, the common cases and the
adversarial suite, and rewrites `docs/evidence/acceptance.json`, carrying the later
verification rounds recorded there forward. The real-data rows run only when the data's
location is given (`OMR_IAU_PDF_DIR`, `OMR_IAU_REFERENCE`, `OMR_IAU_DIR`; `OMR_IAU_KEYS` adds
the scoring with the real keys, `OMR_REMARK_DIR` the parallel run with Remark, recorded as pass
or fail only, its expectations in `OMR_REMARK_EXPECT`); their outputs stay on the evaluation
machine. The targeted reruns of the later rounds were run by hand, as each round says.

## 7. Known gaps

These limits apply to the MVP as measured here.

**Not proven yet**
- **Identity against a real roster** is not done. A parallel run with Remark was performed
  internally as a development check (§5); its figures stay inside the university, and the
  licence review it needs (Q-08) is still open. Scoring on the real exams is proven
  with keys verified by agents (§4.1) and read from the instructors' key sheets, not yet
  confirmed by the instructors in the app.
- **Platforms.**
  - **Tested on:** Linux (every row of this document), and Windows Server 2025 in CI (GitHub's
    `windows-2025` image, `x86_64-pc-windows-msvc`, the C runtime linked statically). On Windows
    this covers the `scripts/check.sh` gate only: not the acceptance run, the long `#[ignore]`d
    rows or the real-data rows, and not yet a Windows 10 or 11 PC. Identical decisions across
    operating systems are not proven yet (§9.4, `docs/IMPLEMENTATION.md`).
  - **Not built:** macOS. There is no installer (spec §24).
  - **Not implemented on Windows yet** (`docs/IMPLEMENTATION.md`): the Windows signals of §14.2
    (mapped network drives, the file-system type, OneDrive and other Cloud Files sync roots; UNC
    paths and sync-client marker files are refused on every OS), owner-only ACLs, DPAPI key
    protection and the audit principal from `USERNAME`.
  - **Desktop app:** has run headless only (the testing backend and software-rendered
    screenshots), never in a real window. Its native file dialogs (the Windows dialog, the XDG
    desktop portal, zenity), the file manager it opens and the startup message box have never
    been shown on a real display: the tests drive a fake host.

**Recognition and review**
- Classical rules only; the learned mark classifier (spec §8.7) is deferred. A light second
  fill can read like erasure residue (the hard-slice error). A sticker or white-out over one
  unfilled IAU bubble reads as blank (a whole answer row whose printed rings are not seen is
  unreadable since the second adversarial round, but a single covered bubble is not).
- **Phone photos** (F1, C3): the camera path rests on two real photos (the CS 522 key) and
  simulated ones; it is held to the scanner's registration gates, and the phone slice of §8.10
  is not calibrated, so every answer on a photographed student sheet goes to review
  (MRK-001 `uncalibrated_capture`, CAP-003 for the batch): photos cost review time, never
  accuracy. A page is taken for a photo from its file (a colour JPEG below 200 ppi, a camera's
  EXIF tags) or its registration (a corrected lattice, anisotropy above the warning level); a
  photo converted to a gray, square, ≥ 200 ppi image with its EXIF removed is treated as a
  scan. At the recognizer level photos are not column-checked: a photo of a sheet folded
  **vertically** through an answer block so that exactly two options are hidden can still be
  misread (3 of 24 such simulated photos, 22 errors); in the app those answers are reviewed
  (student sheets) or confirmed by the instructor against their row crops (key sheets).
  Scanner pages and every horizontal fold or feed stall are refused. Rescan folded sheets flat
  on a scanner.
- **Key sheets found in the stack** by their marker number: a student who bubbles `0000000000`
  or `9999999999` becomes a second key sheet of the version. The hold names every key sheet of
  that version, whether or not they disagree (C4), and the way out (grade again with
  `--key-sheet FILE --key-sheet-marker none`), but if a key file later replaces the key sheets
  (`omr key replace`), that student's page stays out of the results until the exam is graded
  again. Undo reverts review decisions, never a key-sheet
  confirmation: a confirmed key is corrected with `omr key set` or `omr key replace` (audited
  key versions).
- **Recognition configuration:** the photo verification and the row and column checks, then
  the photo residue level of the third round, changed the legacy configuration
  (`omr-recog/legacy-bitonal/3`, a new `config_sha384` in every engine manifest); on the 143
  real pages every measurement and decision is bit-identical to version 1. Its new thresholds
  (row evidence 0.35 at 0.60, the verification margins, the photo residue at the 95th
  percentile of the page's unmarked bubbles) are initial values set on these pages, two real
  photos and simulated sheets.
- WARN and INFO items are counted, not queued. There is no blind verification sample, no
  leases and no two-person rule. Rosterless identities that need confirmation are not queued.
  There is no "duplicate" identity decision: a rescan whose marks agree is resolved
  automatically (COL-003), but two sheets with one student number and different marks
  (IDN-005) can only be given to different students or to "not on roster".
- A copy whose marks differ from the graded one (COL-004) is held out; grading it instead needs
  a regrade without the other copy. IAU sheets are matched as rescans only by an identical,
  fully bubbled student number.
- Own forms: a page whose code cannot be verified cannot be bound to a booklet by hand.
  Permuted forms with a declared item/option map are not accepted yet.
- IAU legacy form: only `cancel_mode = erase_only`; a mirrored scan is held (REG-003), not
  un-mirrored.
- The version backstops apply to exams created since they exist (`omr grade`, the desktop's
  New exam): a sheet bubbling a version the exam does not declare is held (SCO-005
  `undeclared_version`, never scored), and a blank version bubble under a key every version
  shares is scored under it (SCO-005 `identical_forms`, INFO, not queued; CR-16 resolved).
  Projects created before keep their routing when a batch is added or they are regraded: on a
  one-version exam every sheet takes that version whatever its bubble shows; on a
  several-version exam an undeclared letter gets a provisional version (SCO-005 `inferred`),
  and a blank bubble under a shared key is still queued (SCO-005 `inferred`, CR-16) unless the
  exam declares `--forms A`. While key sheets wait for confirmation, a blank version bubble is
  inferred and queued even under identical keys (the confirmation may still make them
  differ).
- **The New-exam proposal** (§3.1.5): its thresholds (5 % and 3 sheets for a version, or
  every sheet of a smaller stack, the 50/70/30/5 % extent rule, 10 % and 3 sheets beyond the count, 10 sheets to infer a key, 60 %
  for a title) are set on synthetic stacks only, not tuned on real ones. A one-column key whose
  B students left the version blank is proposed as one version and those sheets are scored
  against A's key without a flag (no letter is substantial to ask about; SCO-005 `near_chance`
  is not queued). Headerless key rows that start with a bare letter are read as labelled rows;
  when the letter is each row's first answer instead, a count past the rows' answers (typed,
  or the students' row past them that a survey asks about) asks how to read them, and with
  neither the rows stay read as labelled. The survey reads every printed row where grading reads the exam's, so
  its counts are evidence, not the graded reads. The desktop's New-exam card shows the
  proposal and grades exactly what it shows; `omr grade` does not take its defaults from it
  (`docs/IMPLEMENTATION.md`).
- No hot-folder watching and no SVG/PNG sheet output (CR-19). Key sheets (CR-11, §4.1) are
  confirmed with `omr key confirm` or in the desktop app's Setup screen (read, rendered summary,
  confirmation; covered by `shell-slint/tests/req_a11_real.rs` on synthetic key sheets); the
  desktop app has never run in a real window (above).
- **Flag stability** (§12.2) is computed for the exports, before release and on request
  (`ExamProject::flag_stability`, `judge_flags`); it is never on the ≤ 50 ms regrade path, so
  release says "computing…" until it has judged the current run. A key that nobody chose
  (p = 0) has no r_ir, so rule 1 (SCO-001, the release gate) cannot fire on it: such an item is
  flagged "Top group prefers d" (rank 2, SCO-002, not blocking), as the synthetic midterm's
  planted mis-key shows. This follows the §12.2 table as written; whether p = 0 should block
  release is a question for the spec. Rule 2 has no
  correlation, so its CI is a normal interval of the upper-group proportion difference, and the
  CI ends are applied below N = 1,000 too, which makes small classes conservative.

**Migration from Remark Office OMR** (spec §3.5, §16.3, §23.7), each with its workaround
- Only Remark's graded Excel (`.xlsx`) Advanced Grade export is read. Its CSV and SPSS exports,
  version-mapping and scaled-score tables, Remark-compatible exports, and Remark's response
  data as an assessment of its own (TP-154 without the scans) are not built: export the Excel
  reports, and grade the scans here.
- AND keys (`A+C`) are refused, not scored: write `A|C` (either) or void the question.
- A learning objective whose questions the students do not tell apart is refused, never split
  equally on a guess: give its points (`--points`, *Use the suggested points*).
- Imported CLOs are written in domain K, and one objective may cover other items in each
  version: correct `clo.csv`, then `omr clo replace` and `omr clo approve` (Setup's buttons).
- A graded exam's points cannot change (its stored form map): create the exam from the import.
  The Setup grid shows version A's points; a cell that stands for one version's rows says so.
- Booklets are paired with Remark's rows by student number, then by answers: a heuristic, every
  pairing by answers reported as an identity difference. An ID Excel already rounded
  (`2.2E+09`) cannot be recovered: export again with the ID column as text.
- The parallel run holds release only when its differences are queued (`--queue`, *Queue for
  review*); verify items and two-principal adjudication (§13.4) are not built, so a decision is
  the instructor's alone. It compares Remark's per-student objective subtotals, not the class
  learning-objective report.

**Security and privacy** (spec §14.2–§14.4, §19)
- **In place, with tests (§3.2):** the snapshot bound to the audit chain and checked against
  the store on open; signed checkpoints kept outside the project (truncation, re-chained logs,
  edited tables and older copies refused with SYS-011); a signed export manifest; owner-only
  folders and files; sync and network folders refused (SYS-015); a fresh exam ID and a private
  page-code key per exam; masked student numbers, with reveals and crop exports logged;
  reviewer notes erasable.
- **Not encrypted at rest** (spec §19.2): the store, the snapshot and the page-code secret are
  plain owner-only files. Anyone with the user's account or the disk can read them; keep exam
  folders on encrypted university devices.
- **Checkpoints trust this computer.** A project opened on another computer is checked for a
  self-consistent signature only, and deleting the private key folder removes the evidence
  against rolling a project back.
- **No read-access log** (§19.5): reveals and crop exports are audit events, but viewing
  students in the desktop app is not logged.
- **Resource limits:** each input file is read whole (files over 2 GB refused unread, images
  over 100 megapixels refused from their header); streaming ingest, a per-run decode budget and
  a per-page watchdog (§7.4, §22.3–§22.4) are deferred.
- **Housekeeping:** orphaned spool files are not swept on open, and `omr bench` spools its
  copies under the system temp folder by default (`--scratch` sets it).
- **Supply chain:** licences are checked by a census only; `cargo-deny`, the notices file and
  Slint's AboutSlint widget (§5.4) are not in place.
- **Derived data:** the committed blank IAU form image is the per-pixel median of the 143 real
  scans (no legible handwriting remains: 125 non-white pixels outside the printed form); its
  classification by the university's data office is pending (SEC-13).
- **Screenshots:** `omr-desktop --screenshot` renders whatever workspace it is given; only the
  synthetic workspace may be rendered into the repository (SEC-18).

**Other**
- Memory: every input file is held in memory for the whole run (once), plus about 85 KB per
  page.
- Exports: XLSX and HTML cite the audit-chain head, so only the CSV and JSON decision files are
  byte-identical across runs (§2.5).
- GUI: no keymap editor, no drag-and-drop of files onto the window, no menu of recent exams
  folders; an added batch or a roster run is cancelled only by closing the window
  (`docs/IMPLEMENTATION.md`).
