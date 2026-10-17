# Evidence: the OMR Correction MVP against its acceptance rows

This document is for Imam Abdulrahman Bin Faisal University (IAU): the people who decide
whether OMR Correction can grade the university's multiple-choice exams, and the quality
reviewers who rely on its results for NCAAA and ABET reports. It reports what was measured
against the MVP's acceptance rows (`docs/IMPLEMENTATION.md` §1), how each number was
obtained, whether the row passes, and what each number does **not** show.

Every number below comes from one run of `scripts/acceptance.sh` on the code after the
security and code review fixes (§3.2), with the real-data rows and the desktop rows
(`OMR_ACCEPT_A11=1`). That run wrote
[`docs/evidence/acceptance.json`](evidence/acceptance.json), which holds the same figures with
the name of every test that proves each row. Unless a section says otherwise, the data are
**synthetic**: sheets generated with known answers, so the truth is exact by construction.
The 143 real IAU answer sheets are reported separately in §4, as aggregates only.

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
| A11 | Desktop UI | 20 tests (13 on demo data, 7 over the real core: review, release, New exam, key replacement, live grading); screenshots of every screen in both themes | Screens render; scripted review completes | Pass |
| — | Scoring of the real IAU exams | **Not measured**: the answer keys have not been supplied, so the real runs infer the key and their scores are not evaluated (§4) | Scores equal those from the real key | **Not yet proven** |

Real data (143 IAU sheets, §4): **0 silent errors in 4,089 auto-accepted answers**, review rate
**0.171 %**, student numbers **0 wrong** (138 exact, 5 sent to review).

Adversarial verification (§3.1): 15 hostile-input tests through the whole application, all
passing; 11 defects found on the way were fixed, each with a regression test.

Security and code review (§3.2): of 26 findings above "low", 21 are fixed (18 with regression
tests; CR-05, CR-06 and CR-08 were documentation), 3 in part (SEC-11, CR-09, CR-15: the rest is
listed in §7), 1 is deferred (CR-11, a teacher-filled key sheet) and 1 did not reproduce
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
white-out over an *unfilled* bubble on the IAU form still reads as blank: the ring-visibility
check of spec §8.6 is not implemented. WARN and INFO exceptions are counted but not queued.

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
- `crates/shell-slint/tests/req_a11_real.rs` (7 tests over the real `omr-app` backend, synthetic
  IAU sheets): a scripted review that rescores the results exactly as the core does, then
  every item settled and the results released through *Release results…* with the Blackboard
  upload; live
  grading; adding a later batch from the Scan screen; edge projects (no roster, no CLOs, an
  inferred key) and an empty workspace without a panic; **New exam** from the dashboard (a bad
  entry refused with what to type, then graded into a new folder of the workspace with an
  inferred key, and opened); **Replace key…** from Setup (the inferred key replaced and the exam
  rescored); and a project whose snapshot was edited outside the app refused with SYS-011 and
  the remedy.
- The 28 unit tests of the shell (key map, bindings, view-model formatting) run in
  `scripts/check.sh`.

**Screenshots** (synthetic data only, rendered headless by `omr-desktop --screenshot`):
[`design/screens/`](design/screens/) (the demo data, marked DEMO DATA: exams, setup, scan,
review, review-identity, review-blind, results, outcomes, settings; light, dark and
1280 × 800) and [`design/screens/real/`](design/screens/real/) (two synthetic IAU exams graded
through `omr-app`: the same screens plus review-scoring, review-override, results-export,
scan-add and exams-new, the New-exam form), for example
[the review screen](design/screens/real/review-light.png),
[results](design/screens/real/results-light.png) and
[New exam](design/screens/real/exams-new-light.png).

**Result: pass.** **Caveats:** the app has never run in a real window or on Windows (§7): the
tests use Slint's testing backend and the screenshots its software renderer. GUI gaps: no
keymap editor, and the roster and CLO file are chosen when the exam is created (the CLI's
`omr grade` options), not replaced later from the app.

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
| CR-09 | Launching the app without a display printed a raw error | The error says what to do (desktop session, `--screenshot`, or the CLI). Windows and a real window remain untested (§7) | — |
| CR-10 | The section and exam type read from the IAU sheet were discarded | Carried into the exports; a sheet whose section disagrees with the roster is IDN-004 `cross_section` (INFO) | `c13_sheet_section_is_carried_and_checked_against_the_roster` |
| CR-11 | No teacher-filled key sheet | Not built: listed as deferred (IMPLEMENTATION.md §2) | — |
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

**Answer keys were inferred.** The owner has not supplied the answer keys yet, so these runs use
`--infer-key`: each version's key is the most common confident answer per question. Every output
of such a run is marked INFERRED and can never be released. Accuracy below is measured against
the reference labels (what the student marked), which does not depend on the key; scores are
not evaluated.

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
| Throughput (`omr bench`, 4 threads) | **73.2 pages/s** recognition, 67.8 pages/s full pipeline (19.4 and 36.9 pages/s at 1 and 2 threads; 76–89 at 4 threads in earlier, quieter runs) |
| Whole app, 143 pages, store and exports | 2.45 s (58 pages/s) |
| Rescans, refused or held pages | 0: no real page was superseded as a rescan, refused for its resolution or held as a foreign form; each 600-dpi page carries CAP-002 INFO (read from the 200-dpi working copy) |

The whole app sends two more student numbers to review than the recognition-only count
(3 `id_ambiguous`, 2 `id_blank`, from the rosterless IAU identity rules); none was wrong.

**What this does and does not show.** It shows that on this university's real forms, scanners
and students the software made no silent error in 4,089 accepted answers while asking a person
about 7. It does not certify a rate: 4,096 questions bound SER at 7.3 × 10⁻⁴, far above the
contract's 10⁻⁵, and the labels are agent-adjudicated. The sheets come from two courses of one
college.

## 5. A comparison with Remark Office OMR

The owner reported that Remark Office OMR found some of these sheets hard. A fair comparison
needs, for the same five PDFs:
1. **The answer keys** for CS 522 and ARTI 503 (versions A and B), so both systems score
   against the same key.
2. **Remark's output**: the per-question responses it read for every sheet (not only totals),
   its exception or review list, and the template and settings used (Remark version, bubble
   thresholds, bitonal or gray processing).
3. Optionally, Remark's processing time and the number of sheets a person had to look at.

With these, every question where the two systems differ would be adjudicated on a 600-dpi
zoom by two people, and both systems' silent errors and review rates reported side by side.
Agreement between the two systems is never counted as a label (shared errors are invisible,
spec §23.1), and publishing figures against Remark outside the university needs the licence
review the spec names (Q-08).

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

**Reproduce.** `scripts/acceptance.sh` runs everything above and rewrites
`docs/evidence/acceptance.json`. The real-data rows run only when the data's location is given
(`OMR_IAU_PDF_DIR`, `OMR_IAU_REFERENCE`, `OMR_IAU_DIR`); their outputs stay on the evaluation
machine.

## 7. Known gaps

These limits apply to the MVP as measured here.

**Not proven yet**
- **Scoring on real exams.** The real sheets' answer keys have not been supplied, so their keys
  were inferred and their scores are not evaluated (§4). Identity against a real roster and a
  side-by-side comparison with Remark are not done either (§5).
- **Platforms.** Built and tested on Linux only; Windows 10/11, the spec's primary platform,
  and macOS have not been built, and there is no installer (spec §24). The desktop app has run
  headless (the testing backend and software-rendered screenshots), never in a real window.

**Recognition and review**
- Classical rules only; the learned mark classifier (spec §8.7) is deferred. A light second
  fill can read like erasure residue (the hard-slice error). A sticker or white-out over an
  unfilled IAU bubble reads as blank.
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
  un-mirrored. A single-version exam needs `--forms A`, otherwise a blank version bubble is
  queued (SCO-005) although both versions share the key (CR-16).
- No teacher-filled key sheet (CR-11), no hot-folder watching and no SVG/PNG sheet output
  (CR-19).

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
- GUI: no keymap editor; the roster and CLO file are chosen when the exam is created and cannot
  be replaced from the app afterwards.
