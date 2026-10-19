# Implementation plan of record (MVP: walking skeleton + proven core)

This file turns `docs/TECHNICAL_DESIGN.md` (the spec) into the first buildable increment. The spec stays authoritative. Where this MVP narrows or defers something, the spec section is named, so the gap is explicit.

## 1. What the MVP must prove

The **common case** (the spec's "standard slice", §2.4, §23.1):
- **Sheets.** The university's own generated A4 sheets: 20–100 multiple-choice questions with 4–5 options, 1–4 forms (variants), and a bubbled student-ID grid.
- **Capture.** ADF scans at 200–300 dpi, grayscale, delivered as JPEG/PNG/TIFF files or as scanner PDFs.
- **Marking.** Pencil or pen fills, including light fills and erasures; `cancel_mode = erase_only`.
- **Downstream.** A roster, an answer key per form, item→CLO mappings with weights and notes, and NCAAA/ABET-style attainment.

| ID | Capability (spec) | Proof (automated) | Target |
|---|---|---|---|
| A1 | Recognition accuracy with calibrated abstention (§8, §23.1) | Held-out semi-synthetic standard slice (`omr-sim`, truth by construction) | Report SER (silent errors per question) and AR (review rate) with the one-sided 95% bound (3/n at k = 0). Goal: 0 observed silent errors on ≥ 50,000 test questions, AR ≤ 1%. A harder slice (light marks, erasures, strong distortion) is reported separately |
| A2 | Throughput (§2.4, §21.1) | `omr bench` on this 4-vCPU container, JPEG at 200 dpi | ≥ 40 pages/s (the spec's 4-core floor); stretch ≥ 100 |
| A3 | Time to first result (§2.4) | 30 sheets end to end | < 3 s |
| A4 | Regrade latency (§10.1) | Key change on 10,000 sheets × 100 items | ≤ 50 ms |
| A5 | Determinism (§8.11, §23.6) | Same corpus at 1, 2 and 4 threads and in shuffled page order | Byte-identical decisions |
| A6 | Identity (§9) | Roster match from the bubbled ID; national ID/Iqama detection; duplicates; not-on-roster | Correct codes: IDN-004 sub-reasons, IDN-005 |
| A7 | Page code (§6.4) | Encode/decode round trip; foreign or forged sheet | 96-bit HMAC-SHA384 tag verified; IDN-002 on failure |
| A8 | Scoring (§10) | Oracle tests: multi-form mapping, rules, exact rationals, Saudi grade scale | Exact match with hand-computed oracles |
| A9 | Outcomes (§11) | Oracle tests: average, threshold-proportion, Taif per-question, FCAR/EAMU, TP-154 B.1 rows, PLO roll-up | Exact match with hand-computed oracles |
| A10 | Exceptions (§13.1, App. A) | Every raised code exists in Appendix A; `UNREADABLE` is never `EMPTY` | Catalogue test plus targeted tests |
| A11 | Desktop UI (§5.2, ADR-001 S7) | Headless screenshots of the main screens; review flow driven through accessible labels | Screens render; scripted review completes |

### Status (acceptance run of 2026-10-17)

Every row was proven by one run of `scripts/acceptance.sh` on the code after the security and
code review fixes: a release build, all `req_a*` tests including the long runs, the common
cases, the adversarial suite `omr-app/tests/adversarial.rs`, the gated real-data runs, and with
`OMR_ACCEPT_A11=1` the desktop rows (A11, dev profile: not timed). The evidence for the
university, with methods and caveats, is [`docs/EVIDENCE.md`](EVIDENCE.md); the figures, with
the digest of the code that was built, are in
[`docs/evidence/acceptance.json`](evidence/acceptance.json). Not proven yet: scoring on the real
exams (no answer keys supplied then) and any platform other than Linux. The instructors' keys
arrived afterwards: scoring on the two real exams with them, and with their key sheets, is
proven by `omr-app/tests/real_iau_scored.rs` (EVIDENCE §4). A second adversarial verification
then reproduced the photo registration fix (F1), the key sheets and flag stability, fixed six more
defects (a fold or feed stall read rows off, four key-sheet cases, flag stability missing from the
exports) and reran the real-data rows (EVIDENCE §3.1.1, `acceptance.json` `real_data.second_round`).
A third round (EVIDENCE §3.1.2, `real_data.third_round`) made a confident probable mis-key block
release until the instructor decides it, made flag stability depend on content only, sent
answers on photographed student sheets to review, gave key sheets read after grading the audited
path, and re-measured throughput. A fourth round (EVIDENCE §5.1, `real_data.fourth_round`) verified
the Remark import and the parallel run: ten defects fixed, each with a regression test (the
points solver overflowed from about 60 questions, a student whose ID Remark wrote as a word
ended the rows, one stray far-corner cell aborted the process, ID matches ignored the answers);
the parallel run on the real exams was performed internally as a development check and its
figures stay inside the university (spec §3.5, Q-08: the licence review is still open). A fifth
round (EVIDENCE §5.2) made the parallel run compare the exam's own grades and queue its
differences so they hold release, refused undetermined learning-objective points and AND keys,
added the audited approval and replacement of imported CLO mappings, and moved every
expectation tied to the real exams out of the repository.

| ID | Status | Measured (shared 4-vCPU container) |
|---|---|---|
| A1 | Pass | Standard slice: 0 silent errors in 50,000 held-out questions (bound 6.0 × 10⁻⁵), AR 0.50 %; hard slice 1 in 50,000, AR 5.9 % (reported). Real IAU sheets: 0 in 4,089 auto-accepted, AR 0.171 % |
| A2 | Pass | 131.4 pages/s (own forms, 200-dpi JPEG), 88.4 pages/s (IAU form, 600-dpi bitonal PDF), 73.2 pages/s (real IAU PDFs), 4 threads; a 500-page PDF through the whole app at 81.9 pages/s, peak memory +276 MiB (the PDF once plus ≈ 100 KB per page) |
| A3 | Pass | 30 sheets end to end: 0.69 s (own forms), 0.72 s (IAU form) |
| A4 | Pass | Key change 12.8 ms (scoring); review action 22.1 ms, key change 17.1 ms (app); a review action with its audited snapshot save 32.6 ms (156 MB snapshot) |
| A5 | Pass | Decision CSV/JSON files byte-identical at 1, 2, 4 threads and shuffled order (own forms, IAU form, and a hostile batch at 1–4 threads); XLSX/HTML/summary/manifest excluded (wall-clock chain head, timings) |
| A6 | Pass | 21 identity oracle tests; all 14 IDN-004 sub-reasons and IDN-005 |
| A7 | Pass | 14 tests; forged and foreign sheets held with IDN-002 through the whole app |
| A8 | Pass | 17 scoring oracle tests |
| A9 | Pass | 12 outcome oracle tests |
| A10 | Pass | 0 unknown exception codes in 241 source files; UNREADABLE never EMPTY through scoring; 15 adversarial tests give each hostile input its Appendix A code (11 defects fixed) |
| A11 | Pass | `crates/shell-slint/tests/req_a11.rs` (13 tests, demo data) and `req_a11_real.rs` (10 tests over omr-app: review and release, New exam (also with key sheets), key replacement, key sheets read in Setup and confirmed through the audited path, flag stability in Results, live grading, added batches, a tampered project); screenshots in `docs/design/screens/` and `docs/design/screens/real/` (the key-sheet screens `setup-keysheet`, `scan-keysheet` included). Headless only (Linux, and in CI on Windows Server 2025): never run in a real window |

## 2. Scope

**In the MVP:**
- Sheet generation: printable PDF from the template model (§6.1–§6.5), using AprilTag 36h11 corners, row ticks and the QR v3-M page code with the 96-bit tag. A new exam gets a fresh exam ID and page-code key, kept in the user's private key folder, never beside the PDF.
- Semi-synthetic corpus generation (§23.3).
- Image ingest (JPEG/PNG/TIFF) and PDF image-XObject extraction (§7.3).
- The recognition pipeline (§8.2–§8.8), classical cascade (see the deferred list for the CNN).
- Bubbled-ID identity with roster reconciliation (§9.2, §9.5–§9.6, Saudi rules).
- Scoring (§10.1–§10.6), with key sources: a key file, an inferred key (never releasable) and
  teacher-filled key sheets (§10.1: read by the same recognizer, named with `--key-sheet` or
  found in the stack by their marker student number, held until the instructor confirms the
  rendered key summary).
- CLO engine (§11.1–§11.4, §11.7 tables).
- CTT item analysis (§12.1–§12.2) with flag stability (§12.2–§12.3: bootstrap shares and
  percentile CIs of r_ir below N = 1,000, Fisher-z above); a probable mis-key at normal
  confidence (SCO-001 ERROR) is a review item that blocks release until the instructor changes
  the key, accepts a second answer, voids the item or keeps the key with a reason and a note.
- SQLite project store with a SHA-384 hash-chained event log (§14.2–§14.4): the session snapshot bound to the chain (`SnapshotSaved`), projections replayed against their events on open, checkpoints signed with a per-device ECDSA P-384 key and kept outside the project (truncation, re-chained logs and older copies refused with SYS-011), reviewer notes stored by salted reference so they can be erased.
- Privacy controls of §14.2–§14.4 and §19: project folders and exports owner-only (0700/0600), sync and network folders refused (SYS-015), student numbers masked in CLI output unless revealed, reveals and crop exports recorded in the audit log.
- Migration from Remark Office OMR (§3.5 item 3, PAR-09), narrowed (see the deferred list):
  its graded Excel exports become a key file with exact points per version (checked against
  every total Remark reported, and refused where the export does not determine them), the
  learning objectives as CLO mappings "imported, needs review" (approved or replaced later by an
  audited act, `omr clo`), and a version map; the parallel run of §23.7 compares a graded
  exam's own grades with them in a Restricted report and, on request, queues each settled
  difference for review, where it holds release and its decision becomes a field label (Q-08:
  its head-to-head figures stay inside the university). Remark's binary formats are never read.
- CSV/XLSX/HTML exports (§16.3, §17), listed in a manifest signed with the device key; the release step (§13.4: `ResultsReleased`, superseded by any later change) gates the Blackboard grade-center upload.
- The end-to-end pipeline in `omr-app` with rayon parallelism.
- The `omr` CLI.
- The Slint desktop shell (§5.2): an exams folder by default (demo data only with `--demo`, marked on every screen), a New-exam form that grades a folder of scans, setup with key replacement, live import and added batches, the review queue, results with release and exports, and outcomes.

**Deferred, with the spec section that owns each:**
- The CNN/conformal stage of the cascade (§8.7 steps 3–6); the MVP uses calibrated classical features plus hysteresis.
- Handwritten name HTR (§9.4 channel C).
- Typst rendering (§6.2): the MVP renders through SVG behind the same API.
- TWAIN/eSCL capture agents (§7.1) and hot-folder watching; the MVP imports files and folders (`omr grade`, `omr add`, the desktop's New exam and Add scans).
- SVG and PNG sheet output (§6.2); `omr sheet render` writes PDF.
- Packaging, installers and the macOS build (§24). The desktop app has run headless (testing backend and software-rendered screenshots), never in a real window. Built so far: the app icon and version resource in both Windows executables (`omr-winres`, no resource compiler), a GUI-subsystem release build of `omr-desktop.exe` that shows startup errors in a window instead of a console, the operator name from `USERNAME` on Windows, and a Linux desktop entry with its icon in the release archive.
- The Windows platform layer. Windows is built and tested in CI on Windows Server 2025 (MSVC), but these are deferred:
  - the §14.2 Windows signals (`GetDriveTypeW`, the file-system type, Cloud Files sync roots, which include OneDrive Known Folder Move);
  - owner-only ACLs (§19.3);
  - keys protected with DPAPI in the local, not roaming, profile (§19.2);
  - cross-OS decision identity through a pure-Rust `libm` (§9.4);
  - path identity by canonical path (the exam registry compares `std::path::absolute` forms, so another case or an 8.3 short name of the same folder counts as another project).
- The C runtime: the executables link it statically (`.cargo/config.toml`, `+crt-static`) instead of the app-local MSVC runtime of §24.1 and §22.6 row 15; the MSI work (§24.1) settles which one ships.
- Streaming ingest (§7.4) and the per-run decode budget and per-page watchdog (§22.3–§22.4): files are size-checked before they are read and images by their header before they are decoded, but each input file is read whole into memory.
- The read-access log (§19.5 `access_log`): reveals of student numbers and crop exports are audit events; views in the desktop app are not logged.
- Licence enforcement (§5.4: `cargo-deny`, the notices file, Slint's AboutSlint widget).
- Server mode (§15).
- Blackboard/Banner connectors (§16.1–§16.2).
- The DOCX filling of official NCAAA blanks (§11.7): the MVP emits the TP-154 tables as XLSX/CSV/HTML.
- Encryption at rest and the key hierarchy (§19.2); the MVP uses plain SQLite and owner-only files, with hooks noted.
- The AI pack (§18).
- The rest of the Remark migration (§3.5 items 3, 5, 6; §16.3; §23.7): Remark's CSV and SPSS
  exports (only the graded `.xlsx` Advanced Grade export is read); AND keys and partial credit
  (`A+C` is refused, not scored); version-mapping and scaled-score tables; Remark's response
  data as an assessment of its own (TP-154 needs the scans graded here); Remark-compatible
  exports; in the parallel run, the class learning-objective report (each student's objective
  subtotals are compared), verify items and two-principal adjudication of its differences
  (§13.4), and a run in the enclave or on the exam control unit's machines.
- Phone capture (§7.6) and the phone slice's own calibration (§8.10 step 7): live capture,
  bursts and on-device gates are not built. Photos given as files are read (the CS 522 key
  photos read 58 of 58): a page is a camera capture when its file says so (a colour JPEG placed
  below 200 ppi, a camera's EXIF) or its registration does (camera path, one-pitch correction,
  anisotropy above 2 %), which widens the decision band; because the slice is not calibrated,
  every answer on a photographed **student** sheet goes to review (MRK-001
  `uncalibrated_capture`), and key-sheet photos are confirmed by the instructor as every key
  sheet is.
- Parts of the version and key backstops (§10.3; built: undeclared versions held, identical
  versions scored under the shared key, key answers beyond the count named, own sheets' cancel
  mode from the spec, labelled keys never copied, group confirmation only by 10 nats):
  - students' rows beyond the question count (G-3 design, G4): the recognizer decides only
    the exam's rows of a student's sheet, so answers past a count typed too low are not seen
    there (key files and key sheets name theirs; the New-exam survey covers the desktop);
  - SCO-005 `near_chance` stays a score-run notice, not a review item (an owner decision, G6);
  - a batch MRK-018 for confident marks below the last question on many sheets (Fable E5);
  - reusing the survey's page measurements for the grade (each pass reads the pages);
  - `omr grade` keeps its defaults (`--cancel-mode erase_only`, versions A,B without a key
    file that names them): the desktop takes an own sheet's cancel mode from its spec;
  - the IAU student-ID scheme accepts intake years up to 2030 only (§9.2).

## 3. Crates and ownership

The crate map follows spec §4.3 (MVP subset). The dependency rules R1/R2/R6/R8 are enforced by `cargo test -p omr-archtest`.

| Crate | Owns |
|---|---|
| `omr-model` | Shared types and the Appendix A exception catalogue (generated: `scripts/gen_exception_catalogue.py`) |
| `omr-forms` | Template model, layout geometry, page code, rendering (`render` feature) |
| `omr-sim` | Semi-synthetic marks and capture simulation, corpus writer with ground truth (test/bench only, R8) |
| `omr-imaging` | Decode, grayscale normalisation, illumination, sampling, homography/RANSAC |
| `omr-recog` | Page ID, fiducials, registration, features, classification, per-question decisions |
| `omr-identity` | Roster, ID-grid decoding, Saudi ID rules, fusion, assignment |
| `omr-pdf` | Image-XObject extraction from scanner PDFs |
| `omr-scoring` | Keys, forms, policies, L0–L6, grade scales |
| `omr-outcomes` | Frameworks, item→CLO/PLO maps, attainment policies, NCAAA/ABET tables |
| `omr-analytics` | CTT and mis-key detection |
| `omr-store` | SQLite project, SHA-384 event chain |
| `omr-report` | CSV/XLSX/HTML exports and report tables |
| `omr-app` | Pipeline, scheduler, review queue, view-models (the only place with thread pools) |
| `omr-cli` | The `omr` binary |
| `shell-slint` | Desktop UI binding only |
| `omr-archtest` | Architecture fitness tests |

## 4. Engineering rules for every work package

- **Rust and safety.** Rust 2024; `#![forbid(unsafe_code)]` in every crate (R6). Errors use `thiserror`; library code does not `unwrap()` on input-dependent paths.
- **I/O and parallelism (R4).** Engine crates (`omr-imaging`, `omr-forms`, `omr-recog`, `omr-identity`, `omr-scoring`, `omr-outcomes`, `omr-analytics`) do no file or network I/O and create no thread pools. They take bytes or images and return values. Parallelism belongs to `omr-app`.
- **Determinism.** Decisions compare fixed-point values (`omr_model::Darkness`, `LogitMilli`) with hysteresis. No `HashMap` iteration order may leak into outputs. Random generators are seeded explicitly.
- **Exceptions** are only built through `omr_model::Exception::new`/`must`, so every code and severity is checked against Appendix A.
- **Key-blindness (R2).** Recognition never sees a key. Scoring consumes recognition's `Response`s.
- **Tests.**
  - Each crate has unit tests.
  - Properties use `proptest` where invariants exist.
  - Requirement-level tests are named `req_a<N>_...` after the table in §1.
  - Golden fixtures live under `crates/<crate>/tests/golden/` and are changed only together with evidence (R9 spirit).
- **Checks before finishing a package.** These must pass:
  - `cargo fmt --all`;
  - `cargo clippy --workspace --all-targets -- -D warnings` for the crates touched;
  - `cargo test` for the crates touched, plus `-p omr-archtest`.

## 5. Commands

```sh
scripts/check.sh            # fmt check, clippy, all tests, architecture rules
cargo run -p omr-cli -- --help
```
