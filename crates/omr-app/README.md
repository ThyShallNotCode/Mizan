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
exam::{read_key_csv, read_key_file /* + option count */, key_file_forms, read_roster_csv,
       read_roster_xlsx /* calamine */, read_clo_csv, parse_key_entry, clean_roster_id,
       check_roster, decode_text /* UTF-8 ± BOM, UTF-16, Windows-1256 */}
let warnings = def.import_roster(rows)?;   // leading zeros restored under the ID scheme when
                                           // unambiguous; duplicates/damaged IDs refused (R10)

// Grading (scheduler owns the pool; progress events for the live view).
let opts = GradeOptions { threads, project_dir: Some(out.join("project.omrproj")),
                          snapshot_utc, operator, max_pages: None };
let mut exam = ExamProject::grade(&def, &[files_or_folders], &opts, &|p: &Progress| { … })?;
exam.export(&out)?;                  // CSV/XLSX/HTML/JSON + manifest; ExportCreated in the store
let exam = ExamProject::open(&project_dir, operator)?;     // resume (store + snapshot)
let exam = ExamProject::open_read_only(&project_dir)?;     // dashboards (no store lock)
let r: AppendReport = exam.add_scans(&[day2_folder], &opts, &progress)?;  // a later batch:
    // regraded over every original, key and review decisions carried over (SYS-032 on a
    // contradicted human value), new batch + runs in the store; a re-imported file is CAP-011

// Review (§13): queue order, audited actions, incremental rescoring, undo.
exam.queue(Some(Lane::Marks)) -> Vec<&ReviewItem>          // id, lane, kind, code, sub_reason,
                                                           // severity, booklet, page, question,
                                                           // suggestion, fills, group_key, crop
exam.apply(id, Action::Resolve { decision: Decision::Option(1), accepted_suggestion, note })?
exam.apply(id, Action::ApplyToGroup { decision })? / Action::Defer / Escalate / AddToRescan
exam.apply_with_reason(id, action, Some(ReviewReason::with_note("ERASURE_RESIDUE", note)))?
exam.overrides_engine(id, &decision)  // overriding the proposal needs a reason (§13.4);
review::{reason_codes(), reasons_for(kind, lane)}  // §14.4 list, the item's family first
review::remedy(code)                  // what an operator does about a held page (R10)
exam.undo()?                          // compensating events; returns the re-opened item
exam.set_key(form, question, "B")?   // form: 0-based index; question: 1-based (as the mapping
                                      // and the shell); new key version, rescored in place
exam.set_key_with_reason(form, q, "B", Some(ReviewReason::code("KEY_ERROR")))?  // mis-key
exam.replace_key(form_keys, points, reason)?   // the real key after --infer-key: SCO-003
                                               // `inferred_key` clears, inferred forms redone
exam.set_mapping_weight(q, clo, Some(0.5))? / set_mapping_note(q, clo, note)?
exam.crop(id) -> crops::Crop { width, height, rgba, bubbles }   // from the stored original
exam.analytics() / exam.outcomes()    // CTT (§12), attainment + TP-154 (§11)
exam.release(reason)?                 // the instructor's release (§13.4): refused while anything
                                      // blocks it; ResultsReleased; a later change supersedes it
exam.export_blackboard(&path, Some("835932"))?  // Blackboard upload (§16.3), released results only
exam.export_crops(&dir, &ids)? / record_reveal(purpose, n)?  // audited personal-data exports/views

// Inputs and keys (file reads stay here, R4).
define::define(&define::Inputs::new(define::Sheet::parse("iau-ccsit-v1"), 30))?  // the definition
    // from the key, roster and CLO files, as `omr grade` and the desktop's New-exam form read them
define::read_key_path(path, &def)? / define::new_exam_dir(workspace, &def)
keys::{key_dir, device_signer, export_dir, write_private, claim_exam}  // device key, owner-only
    // files, external checkpoints, the exam registry (one project per own-sheet exam)

// View-models for shell-slint (vm.rs; plain Rust, field names of shell `backend.rs`).
exam.summary_vm() / setup_vm() / review_queue_vm(lane) / review_item_vm(id) / history_vm()
exam.results_vm() / outcomes_vm();  vm::list_exams(workspace_dir);  vm::ProgressTracker
```

## Pipeline (§8.1, §4.5, §21.2)

| Step | Where | What |
|---|---|---|
| Ingest | `ingest.rs` | files and folders (walked in name order), each read straight into one shared buffer (never held twice) and refused unread above 2 GB; PDF via `omr-pdf` (pages extracted in parallel, `working_gray` at 200 dpi: bitonal coverage reduction or exact fractional area reduction of gray pages), JPEG/PNG/TIFF via `omr-imaging::decode`, a multi-page TIFF one page per image directory (a damaged chain: the pages before it, then one CAP-001 page for the lost rest); per-page capture class (gray / bitonal / reduced bitonal) and resolution (PDF placement, else snapped from the page size, header dpi distrusted); corrupt, truncated (a JPEG without its end-of-image marker is never decoded), encrypted or zero-byte input → CAP-001 on that page (CAP-004 for JBIG2/MRC), its detail naming the file and page and what to do (R10); every zero-byte file is its own CAP-001, never a duplicate of another; byte-identical file → CAP-011 (the copy ingested first stays the original), the batch continues; a panicking page is CAP-001 |
| Orientation (§8.3) | `engine.rs` | a legacy page whose aspect disagrees with the landscape form (portrait scan) is turned a quarter clockwise before registration, which then tells upright from upside down (90°/180°/270° all read; REG-002 INFO with the turn); an orientation that cannot be decided (upright vs 180° NCC margin < 0.20) is REG-001, never a guess; own forms turn by their tags |
| Blank backs (§7.5) | `pipeline.rs` | a page without a sheet (legacy: not registered; own: no tags or only mirrored ones) and < 0.2 % pixels darker than half the paper level (`omr_imaging::ink_coverage`) is CAP-012 (INFO), `PageStatus::Blank`: dropped, never collated, attributed or queued |
| Resolution (§8.9) | `pipeline.rs` | effective capture resolution: an own form's from its corner tags (scaled back to the scan), else the trusted PDF placement or the page size: < 100 dpi → CAP-002 ERROR, page refused (not graded, queued); 100–150 dpi → CAP-002 WARN on the page; > 400 dpi → CAP-002 INFO (read from the 200-dpi working copy) |
| Spool first (§7.4) | `store.rs` | every original staged (BLAKE3 address, SHA-384) and its page rows written before any page is measured |
| Page stage | `engine.rs`, `pipeline.rs` | one rayon task per page: working resolution (`omr_imaging::resample_area`, any ratio), `LegacyEngine::measure_page` or `OwnFormEngine::measure_page` (tags, Stage A, page code); a provisional decision with the prior baseline feeds the live view (time to first result) |
| Batch stage | `pipeline.rs` | β_p per slice = (capture class × source dpi), order-independent; decisions per page; batch-scope exceptions (CAP-003 `bitonal`, MRK-016) de-duplicated |
| Collation | `pipeline.rs` | booklet ID `bk-<serial>` for a printed serial, else `bk-<first page ID>` (content-derived, stable when a later batch changes the ranks); one booklet per page on single-page sheets; own-form multi-page booklets by the verified serial (generic copies in input order); a missing page is COL-001 per question (pending, never blank, stored); **rescans (§7.4)**: copies of one sheet (same verified serial and page, or on serial-less single-page sheets the same fully bubbled student number) whose marks agree (every answer auto-accepted in both equal, ID columns and bubbled form equal where both are confident) are one reading: the copy with the fewest abstentions is graded (then the first in content order), the others are COL-003 (INFO, `PageStatus::Duplicate`, recognition stored, linked to the graded booklet, not queued); a copy with a printed serial whose marks differ is held out with COL-004 `marks_differ` (ERROR, queued) and never overwrites or duplicates the booklet; serial-less copies whose marks differ stay separate booklets (IDN-005); a verified page code naming a form the exam does not declare is IDN-002 (BLOCKER, held, never scored under another form's key); a page that is not attributed and carries nothing blocking gets IDN-002 (BLOCKER), so no page leaves silently; a blank booklet (MRK-015) is held as pending (never scored 0) until acknowledged |
| Identity (§9) | `omr-identity` | roster-constrained `decode_booklet` + `assign_batch` (duplicates IDN-005, not on roster), or rosterless `decode_rosterless` + `assign_rosterless`; IAU / PNU / KSU / n-digit schemes |
| Form (§10.3) | `infer.rs` | verified page code (priority 1, mapped by the **printed letter**, never by position in the definition), confident bubbled version (2), else key log-likelihood over options + blank + multi (empirical p̂ with ≥ 20 confirmed booklets of the form, leave-one-out for a booklet's own declared form, else the small-class fallback from the key): declared ≠ argmax by ≥ 10 nats → IDN-003 (ERROR, never switched); nothing declared → provisional argmax + SCO-005 `inferred` (ERROR); a verified code naming a form the exam does not declare was held out at collation (IDN-002) |
| Scoring (§10) | `score.rs` | form map in master space (identical keys → shared items; otherwise one item set per form), key version (single / any-of / void → `VoidMode::Drop`), L1 snapshot (pending responses stay `Pending`/`Unreadable`, never blank), `score_prepared`, Saudi grade scale (L6) |
| Outcomes, analytics | `outcomes.rs`, `app.rs` | `compute_attainment` + TP-154 B.1/A.1 on the uncurved run; `omr_analytics::analyze` |
| Queue (§13.2) | `review.rs` | one item per open ERROR/BLOCKER subject: Marks (pending question; crop = the answer row with δ fills, threshold θ_sel and the hysteresis band), Identity (ID grid crop, candidates), Scoring (form inference, inferred key), Capture/notices (refusals, unattributed pages: the detail names the file and page and, through `review::remedy`, what to do, R10); lane per §13.2 table; group key FNV-1a over (code, sub-reason, item/field, suggestion, fill bucket); order: deferred last, severity, code, item, group, top fill, booklet |
| Store (§14) | `store.rs` | exam, template revision (+ the definition without roster or secrets), roster snapshot, grade scale, CLO set, attainment policy, batch, objects, pages, final recognition run (bulk lane, ≤ 50 pages per transaction), booklets, identity run and decisions, exceptions, form map, key and policy versions, mapping set, score run, exports; review actions as `ResponseCorrected` / `ExceptionResolved` / `ExceptionReopened` / `IdentityConfirmed` / `BookletFormConfirmed` / `ReviewItemDeferred` … events |
| Exports | `export.rs` | `results.csv/.xlsx`, `item_analysis.csv/.xlsx`, `test_stats.csv`, `tp154_a1.csv`, `tp154_b1.csv`, `tp154.xlsx`, `report.html`, `review_queue.json`, `booklets.json`, `summary.json`, `key.csv`, `blackboard_grade_center.csv`, `manifest.json` (SHA-384 per file) |
| Blackboard upload | `export.rs` | Blackboard Learn "work offline" grade-center CSV (§16.3, PAR-08; format and sources cited in `blackboard_csv`): `"Username","<course> <title> [Total Pts: <max> Score]"` (+ ` \|<column ID>` for an existing column), one row per student whose result is final (Scored, no open blocking item on the booklet, none at batch/exam scope, no open unattributed page); no names or sections; UTF-8 BOM, CRLF, quoted fields, formula guard; never for an inferred key or a rosterless exam; Restricted label in the manifest (CSV rule) |
| Append (§8.1, §13.4) | `append.rs` | `add_scans`: every stored original (read back verified) + the new files graded again as one batch set (same baselines, collation, serials and queue as grading all at once); a supplied key stays in force (an inferred one is inferred again, a new version when it changed); review states carry over by stable key (kind, code, sub-reason, question, booklet's first page or page); a human value the re-run auto-reads differently is kept and queued as SYS-032 `rerun_disagrees_with_override`, also when a cleaner rescan in the new batch now supersedes the copy the reviewer decided on (the booklet is found through the COL-003 link); store: `batch-N`, `recog-final-N` (parent the previous run), `identity-N`, earlier open exceptions superseded (`RERUN_ACCEPTED`), `BookletRenumbered` for moved serials, `PageAttached` only for pages that moved; earlier review events stay in the history, sealed against undo |
| Key files, rosters | `exam.rs` | key CSV: header or none, one row per question or per version, a single column; BOM, CRLF, UTF-16, Windows-1256; comma, semicolon or tab; lowercase, spaces, `Q7`/`7.`/Arabic-Indic numbers, Arabic headers and option letters; `A\|C`, `A/C`, `A,C`, `AC`, `A or C`; `*`/`void`/`ملغى`; each malformed line refused with its line number and the fix (R10). Rosters: English/Arabic header aliases below title rows; IDs: `.0` dropped and full-precision `E+` expanded (flagged), lossy `2.2E+09` and decimals refused with the Excel fix; leading zeros restored when exactly one padded length fits the ID scheme; duplicate/empty IDs and national IDs refuse the roster (§9.2 rule 4: Appendix A has no roster-import code; the validation report blocks) |
| Reasons (§13.4, §14.4) | `app.rs`, `review.rs` | a group action refused by one member part way (e.g. one student given to two booklets) is recorded and undone, so nothing changes (exit 6, "resolve the items one by one"); `apply_with_reason`: the canonical code on every event of the action (`ResponseCorrected`, `IdentityConfirmed`, `ExceptionResolved`, `BookletFormConfirmed` …), a note only as `note:<ref>#sha384:…`, its text in the store's `review_note` table (salted digest, erasable); overriding the engine's proposal without a reason is refused (exit 6) listing the item family's codes; `OTHER`/`SOLO_SELF_APPROVAL` need a note; confirmations default to the item's code; key changes carry `KEY_ERROR`/`CLERICAL_CORRECTION` (or the given reason) on a published key version |

**Determinism (A5).** Pages are ordered by content (source SHA-384, page index), page IDs
derive from content, booklet serials from that order (or the page code), the batch statistics
are order-independent and every map is a `BTreeMap`: identical outputs at any thread count and
input order (tested on synthetic sheets and checked on the real IAU set).

**Incremental rescoring (A4).** A review action projects only the changed booklet
(`ResponseMatrix::project` of one row), patches its column cells and its L1 hash entry, then
runs `score_prepared`; a key change rescores the prepared matrix without re-projecting. The
incremental result equals a full rebuild (tested).

**Release gate.** `Session::release_blocked` names every reason an exam cannot be released:
an inferred key, open ERROR/BLOCKER items, no key, or a key entry left empty (SCO-003, key
incomplete: the score run is refused, so the exam never reads as releasable without scores;
`omr key set` fills it in or voids it with `*`).

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
| `req_a5_outputs_identical_across_threads_and_input_order` | 1, 2 and 4 threads in name order, 4 threads in a seeded shuffle, 2 threads reversed: every decision file byte-identical (`results.csv`, `item_analysis.csv`, `test_stats.csv`, `tp154_a1.csv`, `tp154_b1.csv`, `booklets.json`, `review_queue.json`, `key.csv`, `blackboard_grade_center.csv`), plus pages, booklets and queue; excluded by design: the XLSX workbooks and `report.html` (they cite the event-chain head, which hashes wall-clock event times), `summary.json` (timings) and `manifest.json` (hashes of those) |
| `tests/acceptance.rs` | A5 on the legacy path (12 PNG sheets + a 600-dpi bitonal PDF of 6, at 1/2/4 threads and shuffled: the same files byte-identical); A7: a forged sheet (another page-code key) and a foreign one (another exam) in a batch are held unattributed with IDN-002 BLOCKER in the Capture lane, release blocked, the genuine booklets read; A10: a 100-question IAU sheet cut off at the right edge has questions 81–100 `Unreadable` (CAP-009), pending in the Marks lane, the total provisional, never blank |
| `req_a4_rescoring_is_incremental_and_fast` (10,000 × 100) | release, best of 5 (acceptance run of 2026-10-17, shared 4-vCPU container at load ≈ 3.5): **22.1 ms** per review action, **17.1 ms** per key change; a review action with its snapshot save (header + one 64-booklet chunk, synced, `SnapshotSaved`) **32.6 ms** for a 156 MB snapshot (0.9 s when the whole snapshot was rewritten); the 50 ms budget (and ≤ 250 ms with the save) is asserted in release builds (debug builds only report) |
| `req_a10_corrupt_inputs_inferred_key_and_view_models` | corrupt JPEG and truncated PDF → CAP-001 (Capture lane), duplicate → CAP-011, batch continues; inferred key = the truth's mode per form/question; SCO-003 `inferred_key` blocks release; all outputs marked; view-models and crops |
| `own_form_sheets_at_300_dpi_as_jpeg_and_in_pdfs_are_read` | 300-dpi JPEG files and 300-dpi PDFs (raw gray image, DCT image) resampled to 200 dpi and read correctly |
| `tests/legacy_synthetic.rs` | the legacy IAU path without real data: the committed blank form rendered with synthetic pencil marks (PNG); bubbled versions map by letter (definition order B, A), 29 questions, rosterless IDs, scores equal a hand count; a one-form exam with blank version bubbles infers its key from every booklet; two booklets with one student number (IDN-005) can never both be resolved to that student |
| `tests/iau_capture.rs` (C1–C3) | IAU sheets turned 0°/90°/180°/270° read upright (REG-002 on the turned ones); a sheet printed over its upside-down copy is REG-001 with nothing auto-accepted; own-form pages at 90°/180°/270° read; blank and bleed-through backs in a duplex PDF and as files → 5 × CAP-012 (INFO), 5 booklets in order, nothing queued; own-form bleed-through backs never shift collation; a 600-dpi bitonal PDF, 300-dpi gray JPEGs and 200-dpi PNGs in one exam: three slices, every page reads exactly as when its slice is graded alone, one CAP-003 |
| `tests/iau_sheets.rs` (C9, C10) | an absent sheet is held (MRK-015, provisional, never 0) with IDN-004 `id_blank`; a partial sheet scores its blanks as unanswered; stray marks in rows 30–100 are ignored (nothing queued, hand-count score); a row with every bubble filled is MRK-002 multi, never credited; a blank ID column (`id_blank`), two bubbles in a column (`id_ambiguous`) and an ID on no roster entry (`not_on_roster`) each propose the roster student, two sheets with one ID raise IDN-005 on both; accepting the proposals settles every identity |
| `tests/instructor_files.rs` (C5, C6) | eight key-file layouts and encodings read the same key; malformed key files fail with their line and the fix; rosters with title rows, Arabic headers and names, BOM, Excel-damaged IDs, semicolons, Windows-1256, XLSX numeric cells; duplicates refused naming the rows; leading zeros restored only when the scheme makes it unambiguous |
| `tests/project_actions.rs` (C5, C7, C8) | the real key after `--infer-key` clears SCO-003 and every score equals the hand count; a mis-key corrected with an audited new key version; an override without a reason, with an unknown code or `OTHER` without a note is refused, with a reason it is in the history and on the events (the note by salted reference, its text in the store's erasable `review_note`); the Blackboard upload holds Username and score only, leaves out students with open items, takes a column ID, exists only for released results (`release`, refused while anything blocks it), disappears at the next export once a later change supersedes the release, and is never written for an inferred key or a rosterless exam |
| `tests/audit_integrity.rs` (SEC-01, SEC-02, SEC-10) | a snapshot edited outside the app refused (SYS-011); a forged snapshot binding caught by the store replay; a decision table edited behind the chain refused; an older copy of the project refused against the device checkpoint; the export manifest signed with the device key; an interrupted save completed on the next open; a damaged or missing snapshot SYS-010 with remediation, never a panic; undoing an identity decision keeps store and exam equal |
| `tests/privacy.rs` (SEC-05, SEC-07, SEC-09) | sync folders refused for projects and exports (SYS-015); package 0700 and every export 0600; review crops an audited, owner-only export with a signed manifest; student numbers masked outside the exports |
| `tests/common_cases.rs` (CR-02, CR-03, CR-10, CR-13) | a 4-option exam on the 5-option IAU sheet (an E mark is MRK-017 `option_not_offered`, the question pending, the run never aborted); absent roster students listed absent; the sheet's section carried and checked against the roster (IDN-004 `cross_section`); an unfinished grading run named with its remedy |
| `tests/append_batch.rs` (C4) | day 1 graded, project reopened, day 2 appended: `results.csv`, `item_analysis.csv`, `test_stats.csv`, `tp154_a1.csv`, `key.csv`, `booklets.json` and the Blackboard upload byte-identical to grading both days at once (pages, booklets, key and queue equal); two batches and runs in the chain, `BookletRenumbered` for every moved serial, chain verifies; a day-1 file imported again is CAP-011 and adds nothing; a resolution taken before the append carries over (results identical to deciding after grading everything) and is sealed against undo |
| `tests/robustness.rs` | forms declared in another order than printed map by the code's letter; a rescanned sheet (same serial, other bytes) is COL-003 when the marks agree and COL-004 `marks_differ` when they differ, never a store failure or a second booklet; a sheet of another template revision is queued (never dropped); a blank booklet is provisional until acknowledged (undo restores the hold); resolved items refuse defer/rescan; `set_key` is 1-based and keeps shared keys in step |

| `tests/adversarial.rs` | hostile inputs, each with its Appendix A code, never a panic or a silent wrong score: zero-byte (each its own CAP-001), truncated PDF/JPEG/PNG, encrypted and mislabelled files → CAP-001 naming the file and the fix; a 3-page TIFF → 3 booklets, a damaged chain → 2 + CAP-001; noise, black, text, every-bubble-filled and own-form pages in a legacy exam (and an IAU sheet in an own-form exam) → REG-001/IDN-002 held; a form-C sheet in an A/B (and an A-only) exam → IDN-002; JPEG at 150 (CAP-002 WARN), 300, 400 dpi read exactly, 75 dpi → CAP-002 ERROR (legacy and own forms, 0 silent errors); rescans and a page fed twice → COL-003 (the copy with fewer abstentions graded), another sheet with the same number → IDN-005; void + `A\|C` key (A or C credited, A+C not, maximum 28); review → key change → undo, append → undo, a group decision refused part way and a cleaner rescan in a later batch (SYS-032): incremental = full rebuild, chain verifies; an incomplete key blocks release; all of it byte-identical at 1/2/3/4 threads, reversed and shuffled; `req_a2_a_500_page_pdf_keeps_memory_bounded_and_throughput` (`#[ignore]`, release): one 500-page own-form PDF (223 MiB) through the whole app at 4 threads: 81.9 pages/s with store (acceptance run of 2026-10-17; 79–82 in other runs), peak memory +223 to +276 MiB (the PDF once plus at most ≈ 85 KB per page; retaining decoded pages would cost ≈ 1.9 GB), 0 silent errors in 19,886 auto-accepted answers |

Real data (`tests/real_iau_e2e.rs`, skipped unless `OMR_IAU_PDF_DIR` and `OMR_IAU_REFERENCE`
are set; aggregates only): the five IAU PDFs as two exams with `--infer-key` (CS 522: 92 pages,
29 questions; ARTI 503: 51 pages, 28 questions):

| Metric | Result |
|---|---|
| Pages | 143 / 143 registered and read |
| Answers | 4,096 questions, 4,089 auto-accepted, **0 silent errors** (95 % bound 3/n = 7.3 × 10⁻⁴), 7 reviews: **AR 0.17 %** (unchanged with the orientation gate and the blank-page test) |
| Orientation, blank backs | every page upright, upright-vs-180° correlation margin ≥ 0.511 (gate 0.20); 0 pages taken for blank backs |
| Student numbers (rosterless, IAU scheme) | 138 exact, 5 flagged (3 `id_ambiguous`, 2 `id_blank`), **0 wrong unflagged** |
| Version / form | 140 declared and correct; 3 inferred (1 unconfident bubble inferred correctly, margin 36.4 nats; 2 blank version bubbles inferred with margins 13.9 and 21.0 nats, SCO-005 for review); no IDN-003: every declared booklet's own form leads by ≥ 6.7 nats leave-one-out (≥ 8.2 before leave-one-out, which flattered the declaration by up to 5 nats) |
| Determinism | CS 522 at 1 thread with the files reversed and ARTI 503 at 2 threads reversed = 4 threads: identical decisions (CSV, JSON, key, queue); XLSX/HTML differ only in the event-chain head they cite (wall-clock event times) |
| Throughput (`omr bench`, release, shared 4 vCPU) | acceptance run of 2026-10-17 (load ≈ 3.5): recognition 19.4 / 36.9 / **73.2 pages/s** at 1 / 2 / 4 threads, full in-memory pipeline 67.8 pages/s at 4 threads (an earlier, quieter run: 22.3 / 44.7 / 89.3); whole app with store, checkpoints and exports 143 pages in 2.45 s = **58 pages/s** (A2 target ≥ 40) |
| Time to first result | 30 sheets end to end in process (store + exports) **0.51 s** (A3 target < 3 s); first provisional page 0.13 s |
| Review action | `omr review resolve` on CS 522 (process start, audited events, rescoring, re-export of 13 files): 59–78 ms; undo 57–64 ms |

## Deviations and open points

- **Forms without a declared cross-form map** get one master item set per form (`A-Q1 …`);
  identical keys share items. Permuted forms with a declared π_f/σ_f (§10.3) are not yet
  accepted from the definition, so item analysis and CLO evidence of different forms are not
  pooled.
- **Legacy sheets** support `cancel_mode = erase_only` only (the validated engine); multi
  review and high-stakes mode are passed to the engines.
- **Batch slices** are (capture class × source dpi) of one grading run; device profiles and
  header sheets (§7.5) are not modelled. An appended batch (`add_scans`) re-fits every slice
  over all pages: before release that is the spec's re-run; after release the frozen model of
  §8.1/§10.8 is not modelled.
- **Appended batches** re-measure every stored page (the per-page records are not stored, so
  a large exam pays its page stage again). An inferred key re-inferred over more booklets is
  a new key version, so its outputs differ from grading everything at once in the version
  number. Unreadable inputs of an earlier batch come back by file name (not path). A human
  form choice the re-run contradicts without an item is kept silently (SYS-032 is raised for
  answers and identities only). A carried decision can no longer be undone (resolved items
  stay resolved), so changing it after an append needs a regrade.
- **Blank backs**: the spec's mirrored-correlation test for bleed-through is replaced by the
  darkness rule (nothing darker than half the paper level) plus "no sheet found" (an own form
  with only mirrored tags is a back); a washed-out real sheet still registers and is graded.
- **Legacy orientation** uses the page aspect (portrait vs the landscape form) and the
  registration's upright/upside-down correlation; the spec's orientation classifier
  (PP-LCNet, §8.3) is not used; mirrored legacy scans (REG-005) are not handled.
- **Roster IDs**: no Appendix A code covers roster import findings; blocking findings refuse
  the roster with the rows to fix (§9.2 rule 4). Leading zeros are restored from the ID scheme
  (the institution's fixed-length rule) rather than a separate import option.
- **Blackboard**: the username is the roster's student ID (no separate username column); the
  REST connector stays deferred (§16.1).
- **Collation** of generic (serial 0) multi-page own forms follows the input order.
- **Queue**: WARN and INFO exceptions are counted, not queued; the blind-verify sample, leases,
  two-person rule and contact-sheet confirmation of `NeedsConfirmation` identities (§13.4,
  §9.7) are not implemented; score-run ERRORs (SCO-008, SCO-005 `near_chance`) appear in the
  readiness checks rather than as items.
- **Store**: the page-code secret is kept owner-only in `project.omrproj/secrets/`, outside the snapshot,
  not encrypted (encryption at rest and the key hierarchy are deferred, IMPLEMENTATION.md §2).
  Location checks (SYS-015) are on for projects and exports. Every change ends with a
  checkpoint signed by the device key (ECDSA P-384, `keys.rs`) and kept outside the project, so
  truncation, a re-chained log or an older copy is refused (SYS-011); a project moved to another
  computer is checked for a self-consistent signature only. Project folders are 0700, exports
  0600.
- **Spool and page stage** do not overlap: originals are staged (fsync) before measuring
  starts (≈ 70–150 ms for the 28 MB of IAU PDFs).
- `max_pages` (benchmarks) selects the first pages in input order.
- **Unverified own-form pages** (page code unreadable, IDN-001/IDN-002) are queued as
  blocking Capture notices but never attached to a booklet: acknowledging one sends the sheet
  to the rescan pull list; a reviewer cannot yet bind such a page to a serial by hand.
- **Rescans** (§7.4): agreeing copies are COL-003 and the copy with the fewest abstentions is
  graded (the registration-residual tie-break of the spec is replaced by content order);
  serial-less copies are matched only by an identical, fully bubbled student number (a copy
  with a doubtful ID column stays a separate booklet with IDN-005). A copy whose marks differ
  (COL-004) is held out; choosing it instead needs a regrade without the other copy.
- **Memory**: every input file is held in memory for the run (one copy), plus ≈ 85 KB of
  measurements per page; decoded pages are not retained. A 5,000-page exam of 200-dpi JPEG
  PDFs needs its file size plus ≈ 0.4 GB; files over 2 GB are refused (CAP-001).
- **Identity review** refuses a student already held by another booklet whose identity is
  settled (IDN-005 by a review decision); the shell's `D` (duplicate) decision has no
  counterpart yet.
- **Unreadable ID-grid / version bubbles** are reflected by the identity (IDN-004) and form
  (SCO-005) items; their bubble-level codes are not stored separately.
- PDF placement resolutions outside 100–1200 dpi are not trusted; the page size decides.
- **Session snapshot.** The derived state lives in `project.omrproj/cache/`: `pages.json`
  (bubble geometry for crops, ≈ 27 KB per page, written once), `session.json` (definition, key,
  queue) and the booklets in chunks of 64 (`booklets-NNNNN.json`, indexed by `booklets.json`);
  a review action rewrites only the chunks it changed. Each save is bound to the audit chain by
  a `SnapshotSaved` event (SHA-384 of every part); on open the snapshot must match it and agree
  with the store's decisions (`audit.rs`), else SYS-011 (edited) or SYS-010 (damaged), never a
  panic; a save interrupted after its event is completed on the next open. A compact per-page
  binary record in the store (§14.3 `page_record`) would replace the JSON for exams of many
  thousand sheets.
