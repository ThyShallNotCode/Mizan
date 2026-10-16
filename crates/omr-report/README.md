# omr-report

Exports and reports (spec §16.3, §17.1–§17.2, §11.7): results CSV/XLSX, item analysis, NCAAA
TP-154 A.1/B.1 tables, the ABET Criterion 3 summary, a self-contained HTML results report and
the SHA-384 export manifest. Facts in, bytes out: every writer takes any `std::io::Write`, the
caller opens files (R4), and output is deterministic (the document date is the snapshot time,
never "now").

## Public API outline

| Area | Items |
|---|---|
| Metadata | `ReportMeta::new(title, snapshot_utc)` with course, term, scope, `classification`, `language`, score run, key/policy versions, roster snapshot, engine manifest, template hash, chain head, `ai_label`, `draft`, `superseded_by`; `provenance()`, `footer_line()`, `compact_footer()`; `Classification::{Public, Restricted (default), Secret, TopSecret}` with `en()`, `ar()`, `label()` ("Restricted / مقيد"), `max()`; `Language::{En, Ar}` |
| Results (§17.2 grade list) | `ResultsTable::from_run(meta, &ScoreRun, &ResponseMatrix, &GradeScale, &RoundingPolicy, &BTreeMap<StudentKey, StudentInfo>)`; `ResultRow { student_id, name, section, booklet, form, status, raw, max, percent, letter, letter_ar, gpa5, gpa4, passing, responses }`; `headers()`, `sheet()`, `about_sheet()`, `write_csv`, `write_xlsx` |
| Item analysis (§12, §17.2) | `ItemAnalysis { meta, report: &AnalyticsReport, show_key }` → `items_sheet`, `tests_sheet`, `flags_sheet`, `write_items_csv`, `write_tests_csv`, `write_xlsx`; helpers `tier_name`, `band_name`, `alpha_band_name`, `flag_label` |
| NCAAA TP-154 (§11.7) | `Tp154 { meta, labels: &Tp154Labels, headers: Headers::{English, Arabic, Both} }` → `a1_sheet(&A1Table)`, `b1_sheet(&B1Table)`, `write_a1_csv`, `write_b1_csv`, `write_xlsx`; `Tp154Labels { en, ar: Tp154Text, source }` (wording is data; `Default` = built-in) |
| ABET (§11.7) | `AbetSummary::build(meta, &OutcomeFramework, &[CourseInput { course, term, crosswalk, attainment, b1 }], &so_values, target, &actions, suppress_n)` → `AbetSoRow { indicators: Vec<AbetIndicator>, result, target, attained, actions }`; `sheet`, `write_csv`, `write_xlsx` |
| HTML (§17.1) | `HtmlReport { results, scale, test, items, show_key, clos: Option<(&AttainmentResult, &CloSet)>, open_exceptions }` → `render() -> String`, `write(w)`; `grade_distribution(&ResultsTable, &GradeScale) -> Vec<GradeBar>` |
| Manifest (§16.3) | `ExportManifest::new(&meta, purpose, pii)`, `add(path, bytes)`, `to_canonical_json()`, `sha384_hex()` (the `manifest_sha384` for `omr-store`'s `record_export`) |
| Table layer | `Sheet { name, title, headers, rows, notes, rtl, widths }`, `Row`, `RowStyle`, `Cell::{Empty, Text, Id, Num, Percent, Int}`; `write_csv(w, &Sheet)`, `write_xlsx(w, &meta, &[Sheet])`; `text::{guard_text, strip_bidi, is_canonical_number, exact_str, float_str, html_escape, option_code, mask_code}` |
| Errors | `ReportError::{Io, Xlsx, Invalid}` with `exception(document)` → SYS-029 (ERROR, Document) |

## Spec sections implemented

- **§16.3 CSV**: UTF-8 with BOM, CRLF, IDs always quoted text, canonical numbers
  (`-?[0-9]+(\.[0-9]+)?`, never prefixed), the formula guard on text columns only (leading
  `=`, `+`, `-`, `@`, TAB, CR → `'`), bidi isolates U+2066–U+2069 stripped. **XLSX**: IDs as
  text, RTL sheet view for Arabic, classification label in the page header and footer, a
  visible banner, document properties (title, comment, keywords, custom `Classification`,
  `ScoreRunId`, `ChainHead`, `RosterSnapshot`, `SnapshotUTC`, `AI`), creation time = snapshot
  time (the snapshot must be RFC 3339 UTC; anything else is `ReportError::Invalid` → SYS-029,
  never a silent "now"), byte-identical output for identical input. **Manifest**: SHA-384 per file with the
  label, purpose and PII flag; canonical bytes with members sorted here (independent of
  `serde_json`'s map order, so a `preserve_order` feature elsewhere in the build cannot change
  the signed hash).
- **§17.1**: deterministic document date; footer provenance (label, score run, key and policy
  versions, roster snapshot, engine manifest, template hash, chain head, UTC time, AI label);
  "draft mappings" and "SUPERSEDED" watermarks; SYS-029 on failure.
- **§17.2**: grade list (raw, percent under the rounding policy, Saudi letter, Arabic letter,
  5- and 4-point GPA, status codes); item analysis (p, D, r_pb, r_ir with CI, band, option
  proportions, ranked flags, key hidden by default, including flag messages that name the
  keyed option) and test summary (summary statistics,
  KR-20/21, α with Feldt CI, SEM).
- **§11.7**: TP-154 A.1 (A+ … F, Denied Entry, In Progress, Incomplete, Pass, Fail,
  Withdrawn; counts and percentages) and B.1 (grouped by NQF domain: # · CLO · related PLO
  code · assessment methods · targeted · actual · comment, footnote, draft watermark) with
  English and Arabic headers; the ABET Criterion 3 summary per SO with indicators, methods,
  N, targets, results with 95 % CIs, verdicts, FCAR flags, the SO roll-up and actions,
  aggregate-only (pseudonymous, §17.6) with small-N suppression: a group below the threshold
  prints `< n` and no result, interval, verdict or FCAR flag, and an SO roll-up resting only on
  such groups is suppressed (SCO-023).
- **HTML report**: one file, inline CSS and SVG, no scripts or external assets; the
  docs/design/DESIGN.md tokens in light and dark (`prefers-color-scheme`); classification
  banner first and last; stat strip; the grade distribution as one teal series with F hatched
  in `error` and the word "Fail", Latin and Arabic letters, counts in mono and a table view;
  CLO meters (μ in `series-1`, π in `series-2`, ink target line, verdict pill and the
  "≈ target (95 % CI)" badge); item table (negative r_ir in `error`, discrimination meter and
  band word, option sparkline, flag pills); student table with `dir="auto"` on every name and
  Arabic cell; `lang`/`dir` from the report language.

## Tests

`cargo test -p omr-report`: unit tests for the text rules, CSV writer, metadata, manifest and
SYS-029; `tests/golden.rs` compares every CSV and the HTML page with `tests/golden/` and asserts
hand-computed values of the fixture (letters, GPAs, percentages, positional responses through a
permuted form, A.1 counts and percentages, B.1 rows, ABET indicator counts, HTML statistics,
no external content); `tests/xlsx.rs` unzips the workbooks and checks shared strings, IDs as
text, numbers as numbers, header/footer label, RTL view, document properties and determinism.
Regenerate goldens with `OMR_UPDATE_GOLDEN=1` only with evidence the change is intended.

## Deviations and deferrals

- **Official wording**: TP-154 headers come from `Tp154Labels::default()`, built from the
  spec's field names and the ETEC glossary term it quotes; the official Arabic and English
  wording belongs to the pinned, signed report-template package (§11.7), which replaces the
  labels as data. The A.1 Arabic status labels reuse `omr-scoring`'s placeholders.
- **Formats**: no DOCX filling of the official NCAAA blanks and no PDF/A (Typst) — deferred by
  docs/IMPLEMENTATION.md §2; the HTML page is the human-readable output. No Hijri computation:
  term labels are printed as supplied.
- **ABET** is a flat Criterion 3 table (per SO and indicator); trends across terms, sampling
  plans and Criterion 4 narrative are not produced. The SO verdict compares the roll-up with
  one institutional target.
- **Signing** of `manifest.json` is left to the caller (`omr-store`'s `CheckpointSigner`);
  `ExportCreated` is recorded by `omr-store::Tx::record_export`.
- Item-analysis statistics are printed with three decimals from `f64` (advisory, §12).
- Only results, item analysis, TP-154 A.1/B.1 and ABET C3 are covered from the §17.2
  catalogue; rank lists, comparative, survey, regrade-impact and operations reports are not.
