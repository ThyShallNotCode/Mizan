# omr-scoring

The scoring engine (spec §10). It is the only crate that reads answer keys (key-blindness
rule R2, §4.3). Every function is pure and deterministic: the same inputs always give the
same `ScoreRun` and Run ID, and a key, policy or weight change never re-runs recognition.

## Public API outline

| Area | Items |
|---|---|
| Exact numbers (§10.2) | `Points = Ratio<i64>`, `Exact = Ratio<i128>`, `pts`, `exact`, `parse_points` ("0.5", ".5", "1/3"), `RoundingPolicy { stage, decimals, strategy, letter_basis }`, `round_exact`, `decimal_string`, `num::{reduce, cmp_exact}` |
| Items and forms (§10.3) | `MasterItem::{sba, true_false, multi_select, score_box}`, `ItemKind`, `SelectMode::{PickN, SelectAll}`, `Form::{identity, permuted}` (π_f = `positions[j].item`, σ_f = `positions[j].option_map`), `FormMap { items, forms, per_form_normalization }`, `FormMap::validate()` → SCO-004/006 |
| Keys (§10.1) | `KeyEntry::{single, any_of, with_credit, set, Open}`, `KeyVersion::{new, revise, printed_key, validate}` → SCO-003/004/027 |
| Key sheets (§10.1) | `key_sheet::SheetCell::{route, key, entry, needs_decision, reason}`: a teacher-filled key sheet's recognised row → `Key` (one confident option, still confirmed by the instructor), `MultiMark` (SCO-027), `Blank` (SCO-003), `NotPrinted` (SCO-004), `Uncertain` (never auto-accepted, SCO-003 until decided); `key_sheet::findings` (the publish codes per row), `key_sheet::combine` (several sheets of one version: rows that differ become uncertain) |
| Policies (§10.4–§10.5) | `PolicyVersion { multi_rules, negative, multi_on_sba, floors, bonus, sections, choice_groups }`, `MultiRule::{Dichotomous, Mtf, Ps1t, Ps50, PlusMinus}`, `NegativeMarking::{Off, Penalty, Formula}`, `MultiOnSba`, `Floors`, `Section { speeded }`, `ChoiceGroup { Best, FirstAttempted }`, `PolicyVersion::lint()` → SCO-019, SCO-004 `choice_group` |
| L1/L2 (§10.1) | `L1Response` (`From<&omr_model::Response>`), `BookletL1::{present, absent}`, `ResponseSnapshot`, `ResponseMatrix::project` (u32 cells, column-major, 4 MB at 10k × 100) |
| Score run (§10.1) | `score(l1, forms, key, policy, adj) -> Result<ScoreRun, ScoringError>`; `prepare(l1, forms) -> Prepared` (key-independent, once per L1 change) + `score_prepared(&prep, …)` (the regrade path); `ScoreRun { run_id, items, l3, counted, students, curve, exceptions }` with `item_score`, `item_state`, `counted`, `max_non_bonus`, `grades(scale, rounding)`, `blocks_release` |
| L3 (§10.1, §10.4) | `CompiledKey::compile` (integer LUTs at a common denominator D), `score_matrix`, `ItemScoreMatrix`, `ItemState` (incl. `NotReached`, SCO-024) |
| L4 (§10.1 order) | `Adjustments { voids, excused, overrides, curve, hold_harmless }`, `VoidMode::{Drop, FullCredit, CreditAny}`, `Curve::{Fit(CurveSpec), Frozen(FrozenCurve)}`, `StudentResult { status, raw, max, step3, step5, final_pct, sections }`, `AssessmentStatus`, `Counted` (step-1 artefacts for CLOs) |
| Curves (§10.6) | `CurveSpec::{AdditiveShift, ProportionalToTop, TwoPointLinear, ZScaling, Root, Lookup}` → `FrozenCurve`; refusals and decreases are SCO-016 |
| L5 (§10.5) | `CoursePolicy { components, excused, missing_work, max_excused_share, extra_credit_cap_pp, rounding, pass_fail }`, `Component { weight, sis_part, required }`, `CourseStudent`, `aggregate_course` (SCO-010), `CourseResult { coursework, final_mark, total_rounded, grade }`, `boundary_list` (SCO-017) |
| L6 (§10.6) | `GradeScale::saudi()` (A+ … F, Arabic letters, 5- and 4-point GPA), `GradeScale::validate` (SCO-029), `StatusCode::{DN, IP, IC, NP, NF, W, E}` with `a1_column()` and DN-as-F GPA, `GradeOutcome` |
| Checks | `run::row_shift_check` (MRK-012), `run::near_chance_check` (SCO-005 `near_chance`); projection raises SCO-008, SCO-031, MRK-013 |
| Statistics helpers | `stats::{wilson_ci, t_interval, fisher_z_ci, f_quantile, t_quantile, normal_quantile, chi2_sf, SplitMix64}` (advisory `f64`, shared with outcomes and analytics) |

## Spec sections implemented

§10.1 (layers L1–L6, L4 evaluation order, Run ID, `ResponseMatrix`, hot loop, pending-never-guessed,
key versions and publish checks), §10.2 (exact numbers, rounding policy, boundary list),
§10.3 (master space, π_f/σ_f, derived printed keys, publish validation, near-chance check,
row shift), §10.4 (SBA with partial and multiple accepted answers, multi-select rules,
true/false, score boxes), §10.5 (negative marking incl. formula scoring, floors, blank,
not reached, confirmed multi on SBA, bonus, voids, sections, choice groups, excused, absent,
overrides, course policy with the SIS split), §10.6 (transforms, freezing, monotonicity,
no-decrease, Saudi scale, status codes, DN), §10.8 edge-case codes listed above.

## Performance (A4)

`tests/regrade.rs::req_a4_regrade_10k_x100` (run with `--release`): a ten-item key change on
10,000 booklets × 100 items over four shuffled forms — validation, key compile, L3, L4,
not-reached, row-shift and near-chance checks, Run ID and L6 letters — takes about 14 ms on
this 4-vCPU container (budget 50 ms). `prepare` (L1 → L2 plus the L1 hash, done once per L1
change) takes about 100–150 ms.

## Deviations and deferrals

- **Numbers.** All points are `num-rational` (`Ratio<i64>` inputs, `Ratio<i128>` aggregates);
  `rust_decimal` is not used (it is not in the lockfile, and rationals represent every
  terminating decimal exactly). The hot loop uses `i64` integers over a run-wide common
  denominator D; if D or a per-booklet sum would overflow `i64`, `CompiledKey::compile`
  returns `ScoringError::Overflow` instead of falling back to `Ratio<i128>` (unreachable for
  realistic keys: D ≤ LCM(1…25)·100 fits easily).
- **Run ID** is SHA-384 over serde-JSON component hashes (field order fixed by the structs),
  not RFC 8785 JCS; the engine identity is `omr-scoring/<version>` until the engine manifest
  hash is wired in by `omr-app`.
- **Publish validation** refuses the whole run on any BLOCKER (SCO-003/004/029) rather than
  scoring the remaining items.
- **Deferred:** numeric grid-in, matching and multiple true–false item types; Rhai/WASM
  custom rules (§10.7, SCO-009); form inference by likelihood/EM (§10.3, IDN-003 and SCO-005
  `inferred`; only `near_chance` is implemented); equating and the Welch ANOVA (SCO-022);
  percentile/rank curve transform; L5 `category_weights` and `drop_lowest`; section weights
  and minima; the scoring-rule simulator UI; post-release governance (SCO-020/021, appeals),
  sittings (§10.9); key-overlap lint SCO-007; Arabic status labels are placeholders until
  the report-template package supplies the official TP-154 wording.
