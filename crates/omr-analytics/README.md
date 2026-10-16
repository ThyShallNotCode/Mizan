# omr-analytics

Item and test analytics (spec §12): classical test theory, ranked mis-key and flawed-item
detection, and small-N handling. Pure functions over one score run's master-space choices
and uncurved L3 item scores; advisory only, they never change a score. Statistics are `f64`
with population variance (§10.2).

## Public API outline

| Area | Items |
|---|---|
| Input | `AnalysisData::from_run(&ScoreRun, &ResponseMatrix)` (voided items and absent students dropped; not-reached cells missing for difficulty and option statistics, 0 for reliability; excused/unselected/other-form cells not presented), `AnalysisData::from_choices` (dichotomous fixtures), `CellStatus`, `ItemInfo { master, … }` (`master` = the score run's master item index; analysis indices shift when voided items are dropped) |
| CTT (§12.1) | `item_stats` → `ItemStats { p, difficulty_flags, d_index, r_pb, r_ir, r_ir_ci, discrimination, low_confidence, tier, options }`, `OptionStats { count, proportion, r, upper, lower, trace (rest-score quintiles), non_functioning }`; `test_stats` (per item-set class) → `TestStats { summary, kr20, kr21, alpha, alpha_ci (Feldt), alpha_if_deleted, alpha_rise_items, sem, band }`; `cronbach_alpha`, `feldt_ci`, `csem_lord`, `pearson` |
| Mis-key rules (§12.2) | `detect` → ranked `ItemFlag { item, master, rule, option, message, confidence, stability }` for rules 1–6 and the multi-select mis-keyed option; `flag_stability` (seeded bootstrap share per flag); `flag_exceptions` (SCO-001 ERROR, SCO-002 WARN; target = master item and master option); `AnalyticsConfig` (all thresholds) |
| Small N (§12.3) | `SmallN::{Minimal, Low, Limited, Moderate, Full}` tiers drive which statistics and rules appear, "possible — insufficient data", suppression of rules 3–4, D hidden below 50; `suppressed(count, n)` ("< 5") |
| One call | `analyze(&data, &cfg)` → `AnalyticsReport { items, tests, flags, exceptions }` (adds SCO-018 for α < .5 or negative, SCO-023 for N < 30) |

## Spec sections implemented

§12 preamble data rules (pooling in master space, item-set classes, not reached, voids),
§12.1 (difficulty and flags, upper–lower D with ties at the cut, uncorrected point-biserial,
corrected item–rest r with Fisher-z CI and bands, option r, distractor functioning and
trace lines, KR-20, KR-21, α, α-if-deleted, Feldt CI, SEM, Lord's CSEM, summary, zero
variance as n/a, negative α reported with the hint), §12.2 (ranked rules 1–6, multi-select
rule, flag stability by bootstrap, codes), §12.3 (tiers, SCO-023, suppression).

## Performance

`tests/scale.rs` (release) reports the synchronous path (`analyze`) at 10,000 examinees ×
100 items: about 55–80 ms on this shared 4-vCPU container (from 650 ms before single-pass
option accumulation, covariance-based α-if-deleted and selection-based quintiles). The
spec's estimate is < 10 ms; see open issues.

## Deviations and deferrals

- Flag stability uses the bootstrap for every N (the N ≥ 1,000 Fisher-z variant is not
  implemented); the app should run it as a background job.
- Rank-based quintiles break ties by examinee order.
- Deferred: Rasch, DIF, marker severity, standard setting, integrity analytics
  (§12.4–§12.7), reference-implementation CI oracles (§12.8), per-printed-position views,
  bootstrap CIs for r_ir and α (Fisher-z and Feldt only), labelled diffs across sittings.
