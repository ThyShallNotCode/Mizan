# omr-identity

Student identity (spec §9): roster snapshots, ID normalisation, per-university student-ID
schemes, Saudi national ID / Iqama detection, roster-constrained decoding of the bubbled ID grid
(`omr_model::IdGridRead`) fused with an optional typed/OCR-read ID string and an optional
page-code binding, rosterless (candidate-number) decoding, and one-sheet-per-student batch
assignment. Engine crate: key-blind (R2), no file or network I/O and no threads (R4); the caller
parses CSV/XLSX and passes records. Exceptions are built only through `omr_model::Exception`
(IDN-004 sub-reasons, IDN-005, IDN-006).

## Public API outline

```rust
// Normalisation (§9.2 rule 3, §9.3).
normalize_id(raw) -> String          // NFKC, Arabic-Indic/Extended → ASCII, bidi stripped, trim, upper; keeps leading zeros
digits_only(raw) -> Option<String>   // digits (spaces/hyphens ignored) or None
normalize_name(raw) -> String        // NFKD, marks/tatweel removed, ٱ ى ة ک ی folded, digits, NFC, case fold
normalize::{name_tokens, token_edit_distance}

// Schemes (§9.2): regex + optional check digit + typed decoder.
StudentIdScheme::new(SchemeSpec { name, version, pattern /* anchored */, check_digit: Option<CheckDigit>,
                                  fields: Vec<FieldRule>, id_space, provisional })?
StudentIdScheme::{iau(), pnu(), ksu(), digits(n, check)}   // IAU: ^2(?P<year>\d{2})(?P<serial>\d{7})$, year 2010–2030
scheme.validate(id) -> Result<DecodedId, SchemeViolation>; scheme.accepts(id); scheme.has_check_digit()
scheme.log_p_fmt(id, &year_counts)   // ∅ prior with the admission-year histogram (§9.5)
CheckDigit::{Luhn, Damm, Mod11 { weights }}.verify(digits)

// National ID / Iqama (§9.2, §9.9 row 8): never a key.
national_id_kind(s) -> Option<NationalIdKind::{Citizen, Resident}>; is_national_id(s); mask_digits(s)
classify_identifier(raw, &scheme) -> IdentifierClass::{StudentId, NationalId(k) /* exact: wrong_identifier */,
                                     NationalIdShaped(k) /* scheme can produce it: note only */, Phone, Unknown}
national_id_column(&values, &scheme) -> ColumnVerdict { values, national_ids, exact, flagged }

// Roster snapshots (§9.2).
RosterRecord { student_key, key_from_numeric_cell, name, name_ar, name_en, section, campus,
               track: Option<Track>, sis_status: Option<SisStatus::{Enrolled, DeniedEntry, Withdrawn, Incomplete, Other}> }
RosterSnapshot::build(records, &scheme, &ImportOptions { pad_to_length }) -> Result<(RosterSnapshot, ImportReport), RosterError::Blocked(report)>
snapshot.{rows(), len(), get(key), sha384(), year_counts(), namesake_groups(), scheme(), diff(&newer) -> RosterDiff}
ImportReport { rows, issues: Vec<ImportIssue>, namesake_groups, national_id_shaped_keys }

// Per-booklet decoding (§9.5, §9.7).
BookletEvidence { grid: Option<IdGridRead>, typed: Option<TypedId { text, source: Typed | Ocr }>,
                  bound_student: Option<StudentKey> /* page-code binding, channel E */ }
decode_booklet(&ev, &roster, &scheme, &PriorContext { session_sections, absent }, &IdentityPolicy) -> IdentityDecision
decode_rosterless(&ev, &scheme, &policy) -> IdentityDecision          // identity_mode = candidate_number
mark_roster_diff(&previous, &mut current) -> bool                     // IDN-004 roster_diff after a new snapshot
IdentityDecision { status: AutoAccepted | NeedsConfirmation | Review, student, candidates: Vec<Candidate>,
                   null_log_posterior_q, logit_q, margin_q, gap_q, read: ReadSummary, channels: [ChannelUse; 3],
                   notes: Vec<IdentityNote>, findings: Vec<Finding>, binding_conflict, exceptions, snapshot_sha384 }
Candidate { student, log_posterior_q, posterior, ll_ab_q, ll_a_q, ll_b_q, ll_e_q,
            relation: Exact | WildcardMatch{columns} | WriterError{kind} | Other, section }
q(nats) -> i64 /* 1/1024 nat */, nats(q) -> f64

// Batch assignment (§9.6).
assign_batch(&[BookletIdentity { booklet, decision, confirmed }], &roster, &prior, &policy) -> BatchOutcome
assign_rosterless(&items) -> BatchOutcome
BatchOutcome { booklets: Vec<BookletAssignment { booklet, assigned: Student | Duplicate | NotOnRoster,
               status, margin_q, decision }>, duplicates: Vec<(StudentKey, Exception /* IDN-005 */)>,
               absent, denied_entry, withdrawn, largest_component }
```

## Model (§9.5)

- **Channel A** (grid): per column `L_c(k) = (1−β−μ)·p_k·Π_{j≠k}(1−p_j) + β·Π_j(1−p_j) + μ·p_k/(K−1)·Σ_{m≠k} p_m·Π_{j∉{k,m}}(1−p_j)`
  from the recognition logits (state-only columns use ±`default_logit`); `UNREADABLE` = 1. Blank
  columns are wildcards; an (n−1)- or (n−2)-digit ID sums over blank placements.
- **Channel B** (typed or OCR string): character edit model (substitution, deletion, insertion;
  `?` unreadable) by the forward recursion; Arabic-Indic and full-width digits are folded.
- **Latent intent AB**: `ℓ_AB(s) = (1−ε_w)·J(id(s)) + ε_w/|N|·Σ_{u∈N(id(s))} ℓ_A(u)ℓ_B(u)` over
  the writer-error neighbourhood (adjacent transposition, substitution, dropped digit), with the
  execution-error term `J = (1−ε_x)ℓ_Aℓ_B + ε_x/2·(ℓ_Aℓ̃_B + ℓ̃_Aℓ_B)` on the direct intent.
- **Channel E**: `1−ε_qr` for the bound student, `ε_qr/(|R|−1)` otherwise; a binding that
  contradicts the handwriting is IDN-006.
- **∅** = `π(∅)·P_fmt(ŷ)·ℓ_AB(ŷ)`; **gap test** `G = log ℓ(ŷ) − max_s log ℓ(s)`: below `G_w − 2` no
  concern; up to `G_∅` (12 nat) `not_on_roster` + `writer_error` unless E supports; beyond,
  `not_on_roster`. **The resolver never snaps a read to the nearest roster entry.**
- **Normaliser.** Without channel B every roster row is scored exactly (channel A's neighbourhood
  sums are computed implicitly in O(n²) per row). With channel B, `block_k` rows plus every roster
  ID within one edit of ŷ or of the written string are scored exactly and the rest is bounded
  (exact A sums × B's maximum), so posteriors can only be understated.
- **Decision** (§9.7): logit ≥ L_id (6.9) and margin ≥ τ_m (4.6) and AB support ≥ θ_f (4.6), each
  with hysteresis h = 0.5 on quantised values; plus a second channel (E) or
  `id_only_auto_accept` with a check-digit scheme. A clean single-channel match is
  `NeedsConfirmation` (contact-sheet confirmation, shadow mode) with no exception. INFO-eligible
  sub-reasons (`id_blank`, `id_ambiguous`, `writer_error`, `channel_conflict`, `wrong_field`,
  `cross_section`) are INFO only on an auto-accepted booklet, otherwise ERROR.
- **A vs B conflict**: each channel's LLR for the best candidate against the other candidates and
  that channel's own best string; a contradiction (≤ −θ_f) is IDN-004 `channel_conflict`.
- **Batch**: costs `−log P(s|O)` in 1/1024 nat, a private ∅ column per booklet (+λ_new 2 nat), a
  dup column per student (+λ_dup 5 nat), exact pruning, union-find components, an exact integer
  Hungarian solve per component (rows by booklet ID, columns by key), margins by re-solving.
  Confirmed pairs are fixed. Dup columns and strong competing claims raise IDN-005 (Student);
  `assignment_changed`, `assigned_to_absent` (absent, DN, W) and batch-margin `low_margin` are
  added; DN students are listed apart, never absent.

## IAU real-data check (`tests/real_iau_identity.rs`)

Skips unless `OMR_IAU_REFERENCE` (the adjudicated `reference.json`) is set; `OMR_IAU_OUT`
receives the aggregate report. Only counts are printed; assertion messages carry no values.

| Check | Result |
|---|---|
| Complete reference student numbers satisfying the IAU scheme | 141/141 (1 partially blank grid, 1 fully blank) |
| Of those, Luhn-valid national-ID shapes (chance ≈ 10 %) | 14: classified `NationalIdShaped` (note), never `wrong_identifier` |
| Roster import per course (91 and 50 students) | clean; no national-ID column flag |
| Pages decoding to their own number | 141/141 (`NeedsConfirmation`: no second channel on this legacy sheet); the 2 incomplete grids go to review (`id_blank`) |
| Handwritten ID (two readers merged, disagreements → `?`) | 132 agree, 0 silent disagreements, 9 unreadable; on the 2 incomplete grids 1 consistent, 1 flagged `channel_conflict` |
| Batch assignment | every complete page assigned its own number (one tie with an incomplete page claiming the same student → IDN-005, both to review); 1 not-on-roster (incomplete page) |
| Leave-one-out (student removed from the roster) | 141/141 `not_on_roster` (61 within one writer error of another roster ID), 0 matched to another student |
| Synthetic national IDs and Iqamas (286) | 286/286 refused in roster mode, rosterless mode and roster import; digits masked |
| Rosterless mode on the complete numbers | 141/141 accepted as keys |

Decode cost (release, one booklet): 300 students ≈ 1.2–1.7 ms (grid) / 4–6 ms (grid +
handwriting); 5,000 students ≈ 12–19 ms / 22–31 ms (§9.4 budget 10–40 ms per identity page).

## Tests

Unit tests per module; `tests/a6_identity.rs` (`req_a6_*`, `req_a10_*`): exact match, page-code
auto-accept and IDN-006, blank/double/unreadable wildcards (ERROR vs INFO), never-snap for
substitutions/transpositions/far reads, national ID/Iqama/phone detection and masking, typed
agreement and conflict, typed resolution of a blank column, rosterless mode, batch duplicates
and not-on-roster, fixed pairs, DN/W lists, `assignment_changed`, batch-order invariance,
blocked-scoring conservativeness, `roster_diff`, catalogue conformance, and a never-nearest-
neighbour property; `tests/perf_identity.rs` (ignored timing).

## Deviations and open points

- The IAU scheme is **provisional**: inferred from 141 real numbers (`2 YY NNNNNNN`, YY = 19–22);
  the registrar must confirm the format and whether a check digit exists. Without a check digit
  `id_only_auto_accept` cannot be enabled (§9.7), so on legacy IAU sheets (no page code, name HTR
  deferred) identity is always `NeedsConfirmation` or `Review`.
- Channel B is an ID string (typed or OCR text) scored by a character edit model, not the
  per-box glyph CNN/CTC of §9.4 (deferred); the script latent is not modelled.
- Channel C (handwritten names, Fellegi–Sunter, skeletons, name variants) is deferred; names are
  normalised and namesake groups recorded, but `namesake` is never raised.
- Cross-track rules need the batch header (§7.5), which is not an input yet; `cross_section` uses
  `PriorContext::session_sections` only.
- Case folding uses `to_lowercase` plus ß → ss instead of ICU4X full folding.
- Execution error is applied to the direct intent only, not inside the writer-error sum.
- LAP ties (two booklets equally certain of one student) are broken by booklet order; both
  booklets still carry IDN-005 and go to review.
- Calibration (β, μ, ε, T) and LTT certification (§9.7) are not part of this crate; all
  thresholds are the spec's initial values.
