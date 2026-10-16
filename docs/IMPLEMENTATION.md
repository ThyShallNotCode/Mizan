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

## 2. Scope

**In the MVP:**
- Sheet generation: SVG, PNG and PDF from the template model (§6.1–§6.5), using AprilTag 36h11 corners, row ticks and the QR v3-M page code with the 96-bit tag.
- Semi-synthetic corpus generation (§23.3).
- Image ingest (JPEG/PNG/TIFF) and PDF image-XObject extraction (§7.3).
- The recognition pipeline (§8.2–§8.8), classical cascade (see the deferred list for the CNN).
- Bubbled-ID identity with roster reconciliation (§9.2, §9.5–§9.6, Saudi rules).
- Scoring (§10.1–§10.6).
- CLO engine (§11.1–§11.4, §11.7 tables).
- CTT item analysis (§12.1–§12.2).
- SQLite project store with a SHA-384 hash-chained event log (§14.2–§14.4).
- CSV/XLSX/HTML exports (§16.3, §17).
- The end-to-end pipeline in `omr-app` with rayon parallelism.
- The `omr` CLI.
- The Slint desktop shell (§5.2) for setup, import, review queue, results and outcomes.

**Deferred, with the spec section that owns each:**
- The CNN/conformal stage of the cascade (§8.7 steps 3–6); the MVP uses calibrated classical features plus hysteresis.
- Handwritten name HTR (§9.4 channel C).
- Typst rendering (§6.2): the MVP renders through SVG behind the same API.
- TWAIN/eSCL capture agents (§7.1); the MVP uses hot folders and file import.
- Server mode (§15).
- Blackboard/Banner connectors (§16.1–§16.2).
- The DOCX filling of official NCAAA blanks (§11.7): the MVP emits the TP-154 tables as XLSX/CSV/HTML.
- Encryption at rest and the key hierarchy (§19.2); the MVP uses plain SQLite, with hooks noted.
- The AI pack (§18).
- Phone capture (§7.6).

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
