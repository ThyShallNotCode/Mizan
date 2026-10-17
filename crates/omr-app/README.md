# omr-app

Application services (spec §4.3, §4.5): the exam definition, ingest, the grading pipeline and
its scheduler, the review queue with audited actions and incremental rescoring, exports and
the view-models of the desktop shell. This is the only library crate that creates thread
pools (rayon) and does file I/O (rule R4); engines below it stay pure.

## Public API outline

```rust
// Exam definition (serde; persisted in the project store and the session snapshot).
let mut def = ExamDefinition::new(title, TemplateChoice::Legacy("iau-ccsit-v1".into())
    /* or TemplateChoice::Generated(Box<TemplateSpec>) + page_code_key_hex */,
    questions, options, forms /* ["A","B"] */, CancelMode::EraseOnly /* mandatory */);
def.multi_review = MultiReviewPolicy::Never; def.high_stakes = false;
def.key = KeySource::Provided { forms: vec![FormKey { form, answers /* "B", "A|C", "*", "" */ }] }
       /* | KeySource::Inferred (explicit opt-in) | KeySource::None */;
def.points = vec!["1".into(); n];             // exact decimals
def.roster = Some(vec![RosterStudent { id, name, name_ar, section, numeric_cell }]);
def.id_scheme = IdSchemeChoice::Iau;          // | Pnu | Ksu | Digits(n)
def.outcomes = Some(OutcomeSetup { clos, mapping: vec![CloMapping { question, clo, weight, note }],
                                   split, policy: PolicyChoice::Default });
def.validate()?;
exam::{read_key_csv, read_roster_csv, read_roster_xlsx /* calamine */, read_clo_csv, parse_key_entry}

// Grading (scheduler owns the pool; progress events for the live view).
let opts = GradeOptions { threads, project_dir: Some(out.join("project.omrproj")),
                          snapshot_utc, operator, max_pages: None };
let mut exam = ExamProject::grade(&def, &[files_or_folders], &opts, &|p: &Progress| { … })?;
exam.export(&out)?;                  // CSV/XLSX/HTML/JSON + manifest; ExportCreated in the store
let exam = ExamProject::open(&project_dir, operator)?;     // resume (store + snapshot)
let exam = ExamProject::open_read_only(&project_dir)?;     // dashboards (no store lock)

// Review (§13): queue order, audited actions, incremental rescoring, undo.
exam.queue(Some(Lane::Marks)) -> Vec<&ReviewItem>          // id, lane, kind, code, sub_reason,
                                                           // severity, booklet, page, question,
                                                           // suggestion, fills, group_key, crop
exam.apply(id, Action::Resolve { decision: Decision::Option(1), accepted_suggestion, note })?
exam.apply(id, Action::ApplyToGroup { decision })? / Action::Defer / Escalate / AddToRescan
exam.undo()?                          // compensating events; returns the re-opened item
exam.set_key(form, question, "B")?   // form: 0-based index; question: 1-based (as the mapping
                                      // and the shell); new key version, rescored in place
exam.set_mapping_weight(q, clo, Some(0.5))? / set_mapping_note(q, clo, note)?
exam.crop(id) -> crops::Crop { width, height, rgba, bubbles }   // from the stored original
exam.analytics() / exam.outcomes()    // CTT (§12), attainment + TP-154 (§11)

// View-models for shell-slint (vm.rs; plain Rust, field names of shell `backend.rs`).
exam.summary_vm() / setup_vm() / review_queue_vm(lane) / review_item_vm(id) / history_vm()
exam.results_vm() / outcomes_vm();  vm::list_exams(workspace_dir);  vm::ProgressTracker
```

## Pipeline (§8.1, §4.5, §21.2)

| Step | Where | What |
|---|---|---|
| Ingest | `ingest.rs` | files and folders (walked in name order); PDF via `omr-pdf` (pages extracted in parallel, `working_gray` at 200 dpi: bitonal coverage reduction or exact fractional area reduction of gray pages), JPEG/PNG/TIFF via `omr-imaging::decode`; per-page capture class (gray / bitonal / reduced bitonal) and resolution (PDF placement, else snapped from the page size, header dpi distrusted); corrupt input → CAP-001 on that page (CAP-004 for JBIG2/MRC), byte-identical file → CAP-011, the batch continues; a panicking page is CAP-001 |
| Spool first (§7.4) | `store.rs` | every original staged (BLAKE3 address, SHA-384) and its page rows written before any page is measured |
| Page stage | `engine.rs`, `pipeline.rs` | one rayon task per page: working resolution (`omr_imaging::resample_area`, any ratio), `LegacyEngine::measure_page` or `OwnFormEngine::measure_page` (tags, Stage A, page code); a provisional decision with the prior baseline feeds the live view (time to first result) |
| Batch stage | `pipeline.rs` | β_p per slice = (capture class × source dpi), order-independent; decisions per page; batch-scope exceptions (CAP-003 `bitonal`, MRK-016) de-duplicated |
| Collation | `pipeline.rs` | one booklet per page on single-page sheets; own-form multi-page booklets by the verified serial (generic copies in input order); a missing page is COL-001 per question (pending, never blank, stored); a second page with the same verified serial and template page (a rescan that is not byte-identical) is held out with COL-004 (ERROR) instead of overwriting the first copy or duplicating the booklet; a page that is not attributed and carries nothing blocking gets IDN-002 (BLOCKER), so no page leaves silently; a blank booklet (MRK-015) is held as pending (never scored 0) until acknowledged |
| Identity (§9) | `omr-identity` | roster-constrained `decode_booklet` + `assign_batch` (duplicates IDN-005, not on roster), or rosterless `decode_rosterless` + `assign_rosterless`; IAU / PNU / KSU / n-digit schemes |
| Form (§10.3) | `infer.rs` | verified page code (priority 1, mapped by the **printed letter**, never by position in the definition), confident bubbled version (2), else key log-likelihood over options + blank + multi (empirical p̂ with ≥ 20 confirmed booklets of the form, leave-one-out for a booklet's own declared form, else the small-class fallback from the key): declared ≠ argmax by ≥ 10 nats → IDN-003 (ERROR, never switched); nothing declared → provisional argmax + SCO-005 `inferred` (ERROR); a verified code naming a form the exam does not declare → SCO-005 `code_form_not_in_exam` |
| Scoring (§10) | `score.rs` | form map in master space (identical keys → shared items; otherwise one item set per form), key version (single / any-of / void → `VoidMode::Drop`), L1 snapshot (pending responses stay `Pending`/`Unreadable`, never blank), `score_prepared`, Saudi grade scale (L6) |
| Outcomes, analytics | `outcomes.rs`, `app.rs` | `compute_attainment` + TP-154 B.1/A.1 on the uncurved run; `omr_analytics::analyze` |
| Queue (§13.2) | `review.rs` | one item per open ERROR/BLOCKER subject: Marks (pending question; crop = the answer row with δ fills, threshold θ_sel and the hysteresis band), Identity (ID grid crop, candidates), Scoring (form inference, inferred key), Capture/notices (refusals, unattributed pages); lane per §13.2 table; group key FNV-1a over (code, sub-reason, item/field, suggestion, fill bucket); order: deferred last, severity, code, item, group, top fill, booklet |
| Store (§14) | `store.rs` | exam, template revision (+ the definition without roster or secrets), roster snapshot, grade scale, CLO set, attainment policy, batch, objects, pages, final recognition run (bulk lane, ≤ 50 pages per transaction), booklets, identity run and decisions, exceptions, form map, key and policy versions, mapping set, score run, exports; review actions as `ResponseCorrected` / `ExceptionResolved` / `ExceptionReopened` / `IdentityConfirmed` / `BookletFormConfirmed` / `ReviewItemDeferred` … events |
| Exports | `export.rs` | `results.csv/.xlsx`, `item_analysis.csv/.xlsx`, `test_stats.csv`, `tp154_a1.csv`, `tp154_b1.csv`, `tp154.xlsx`, `report.html`, `review_queue.json`, `booklets.json`, `summary.json`, `key.csv`, `manifest.json` (SHA-384 per file) |

**Determinism (A5).** Pages are ordered by content (source SHA-384, page index), page IDs
derive from content, booklet serials from that order (or the page code), the batch statistics
are order-independent and every map is a `BTreeMap`: identical outputs at any thread count and
input order (tested on synthetic sheets and checked on the real IAU set).

**Incremental rescoring (A4).** A review action projects only the changed booklet
(`ResponseMatrix::project` of one row), patches its column cells and its L1 hash entry, then
runs `score_prepared`; a key change rescores the prepared matrix without re-projecting. The
incremental result equals a full rebuild (tested).

## Inferred key mode

`KeySource::Inferred` is an explicit opt-in (`omr grade --infer-key`) for runs without a key:
each form's key is the modal **confident** (auto-accepted single) response per question over
the booklets that declare that form (on a one-form exam: every booklet, since a lone version
is rarely bubbled); a question without one is voided. It never pretends to be
a key:
- the run raises **SCO-003** (BLOCKER, Exam, sub-reason `inferred_key`): Appendix A's "key
  missing or incomplete" fits, because the real key is missing; BLOCKER/Exam blocks release
  for every student, and `Session::release_blocked` reports "key INFERRED … never releasable"
  even after the item is acknowledged;
- every output says so: an extra first header row in every CSV, the title of every XLSX and
  HTML document plus a banner, a `banner` member in every JSON, `-INFERRED` in every file
  name, `key_status` "Key v1 · INFERRED" in the view-models;
- form inference for undeclared booklets uses the inferred keys (SCO-005 `inferred` remains).

## Evidence

Synthetic (committed, `tests/synthetic_e2e.rs`, no real data; `omr-sim` is a dev-dependency):

| Test | Result |
|---|---|
| `req_a1_a8_synthetic_sheets_grade_end_to_end`: 24 own-form sheets (40 × 4, two forms, roster, CLOs), standard profile, 200-dpi JPEG | 960 questions: 954 auto-accepted, **0 silent errors**, 0 review-truth auto-accepted, AR 0.63 %; forms and students exact; every review item resolved through the API with the truth → every score, percentage and Saudi letter equals the hand computation from the truth and the key; key change and mapping edits; undo; store chain verifies |
| `req_a5_outputs_identical_across_threads_and_input_order` | 1 thread in name order vs 4 threads in shuffled order: identical CSVs, `booklets.json`, key, decisions and queue |
| `req_a4_rescoring_is_incremental_and_fast` (10,000 × 100) | release: **20.6 ms** per review action, **26.0 ms** per key change (shared 4-vCPU container under load; budget 50 ms) |
| `req_a10_corrupt_inputs_inferred_key_and_view_models` | corrupt JPEG and truncated PDF → CAP-001 (Capture lane), duplicate → CAP-011, batch continues; inferred key = the truth's mode per form/question; SCO-003 `inferred_key` blocks release; all outputs marked; view-models and crops |
| `own_form_sheets_at_300_dpi_as_jpeg_and_in_pdfs_are_read` | 300-dpi JPEG files and 300-dpi PDFs (raw gray image, DCT image) resampled to 200 dpi and read correctly |
| `tests/legacy_synthetic.rs` | the legacy IAU path without real data: the committed blank form rendered with synthetic pencil marks (PNG); bubbled versions map by letter (definition order B, A), 29 questions, rosterless IDs, scores equal a hand count; a one-form exam with blank version bubbles infers its key from every booklet; two booklets with one student number (IDN-005) can never both be resolved to that student |
| `tests/robustness.rs` | forms declared in another order than printed map by the code's letter; a rescanned sheet (same serial, other bytes) is COL-004, not a store failure; a sheet of another template revision is queued (never dropped); a blank booklet is provisional until acknowledged (undo restores the hold); resolved items refuse defer/rescan; `set_key` is 1-based and keeps shared keys in step |

Real data (`tests/real_iau_e2e.rs`, skipped unless `OMR_IAU_PDF_DIR` and `OMR_IAU_REFERENCE`
are set; aggregates only): the five IAU PDFs as two exams with `--infer-key` (CS 522: 92 pages,
29 questions; ARTI 503: 51 pages, 28 questions):

| Metric | Result |
|---|---|
| Pages | 143 / 143 registered and read |
| Answers | 4,096 questions, 4,089 auto-accepted, **0 silent errors** (95 % bound 3/n = 7.3 × 10⁻⁴), 7 reviews: **AR 0.17 %** |
| Student numbers (rosterless, IAU scheme) | 138 exact, 5 flagged (3 `id_ambiguous`, 2 `id_blank`), **0 wrong unflagged** |
| Version / form | 140 declared and correct; 3 inferred (1 unconfident bubble inferred correctly, margin 36.4 nats; 2 blank version bubbles inferred with margins 13.9 and 21.0 nats, SCO-005 for review); no IDN-003: every declared booklet's own form leads by ≥ 6.7 nats leave-one-out (≥ 8.2 before leave-one-out, which flattered the declaration by up to 5 nats) |
| Determinism | CS 522 at 1 thread with the files reversed and ARTI 503 at 2 threads reversed = 4 threads: identical decisions (CSV, JSON, key, queue); XLSX/HTML differ only in the event-chain head they cite (wall-clock event times) |
| Throughput (`omr bench`, release, 4 vCPU shared with other builds) | recognition 23.4 / 44.8 / **85.7 pages/s** at 1 / 2 / 4 threads; full in-memory pipeline 88.4 pages/s at 4 threads; `omr grade` end to end (process start, spool, store, exports) 143 pages in 2.13 s = **67 pages/s** (A2 target ≥ 40) |
| Time to first result | 30 sheets end to end (store + exports) **0.50 s** (A3 target < 3 s); first provisional page 0.12 s |
| Review action | `omr review resolve` on CS 522 (process start, audited events, rescoring, re-export of 13 files): 60 ms; undo 56 ms |

## Deviations and open points

- **Forms without a declared cross-form map** get one master item set per form (`A-Q1 …`);
  identical keys share items. Permuted forms with a declared π_f/σ_f (§10.3) are not yet
  accepted from the definition, so item analysis and CLO evidence of different forms are not
  pooled.
- **Legacy sheets** support `cancel_mode = erase_only` only (the validated engine); multi
  review and high-stakes mode are passed to the engines.
- **Batch slices** are (capture class × source dpi) of one grading run; device profiles and
  header sheets (§7.5) are not modelled; re-finalisation after more pages is a new run.
- **Collation** of generic (serial 0) multi-page own forms follows the input order.
- **Queue**: WARN and INFO exceptions are counted, not queued; the blind-verify sample, leases,
  two-person rule and contact-sheet confirmation of `NeedsConfirmation` identities (§13.4,
  §9.7) are not implemented; score-run ERRORs (SCO-008, SCO-005 `near_chance`) appear in the
  readiness checks rather than as items.
- **Store**: the page-code secret is kept in the session snapshot (encryption at rest and the
  key hierarchy are deferred, IMPLEMENTATION.md §2); location checks (SYS-015) are off for the
  output folder; no checkpoint signer is configured.
- **Spool and page stage** do not overlap: originals are staged (fsync) before measuring
  starts (≈ 70–150 ms for the 28 MB of IAU PDFs).
- `max_pages` (benchmarks) selects the first pages in input order.
- **Unverified own-form pages** (page code unreadable, IDN-001/IDN-002) are queued as
  blocking Capture notices but never attached to a booklet: acknowledging one sends the sheet
  to the rescan pull list; a reviewer cannot yet bind such a page to a serial by hand.
- **Rescans** (COL-004): the first copy in canonical (content) order is graded and the other
  is held out; choosing the other copy needs a regrade without the first.
- **Identity review** refuses a student already held by another booklet whose identity is
  settled (IDN-005 by a review decision); the shell's `D` (duplicate) decision has no
  counterpart yet.
- **Unreadable ID-grid / version bubbles** are reflected by the identity (IDN-004) and form
  (SCO-005) items; their bubble-level codes are not stored separately.
- PDF placement resolutions outside 100–1200 dpi are not trusted; the page size decides.
- **Session snapshot.** The derived state lives in `project.omrproj/cache/`: `pages.json`
  (bubble geometry for crops, ≈ 27 KB per page, written once) and `session.json` (booklets,
  key, queue; rewritten after every review action, ≈ 5 KB per booklet). The store remains the
  audit record; a compact per-page binary record in the store (§14.3 `page_record`) would
  replace both for exams of many thousand sheets.
