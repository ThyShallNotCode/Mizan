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
| Mis-key rules (§12.2) | `detect` → ranked `ItemFlag { item, master, rule, option, message, confidence, stability }` for rules 1–6 and the multi-select mis-keyed option; `flag_exceptions` (SCO-001 ERROR, SCO-002 WARN; target = master item and master option); `AnalyticsConfig` (all thresholds) |
| Flag stability (§12.2, §12.3) | `stability_job(&data, &flags, &cfg)` → `StabilityReport { method: Bootstrap \| FisherZ, n, resamples, run_id, seed, flags: Vec<FlagStability { item, master, label, rule, option, share, share_mc, ci_ends: (lower, upper), low_confidence }>, item_ci: Vec<ItemCi { item, master, label, ci, defined }> }` for every rank 1–4 flag, and every item's bootstrap percentile CI of r_ir — a separate job (pure, deterministic, seeded from the data's content: `content_seed`); `apply_stability(&mut report, s)` merges it (share into `ItemFlag::stability`, low-confidence flags to `Confidence::Low` with "— low confidence (proposed): fires in 31 % of 1,000 bootstrap resamples; fails at the upper end of its 95 % CI" appended, SCO-001/002 details rebuilt, `AnalyticsReport::stability` set); `apply_to_flags`; `FlagStability::describe`; `analyze_with_stability` (batch exports, tests); `flag_stability(&data, &mut flags, &cfg, b)` (fixed `b`, kept for callers) |
| Small N (§12.3) | `SmallN::{Minimal, Low, Limited, Moderate, Full}` tiers drive which statistics and rules appear, "possible — insufficient data", suppression of rules 3–4, D hidden below 50; `suppressed(count, n)` ("< 5") |
| One call | `analyze(&data, &cfg)` → `AnalyticsReport { items, tests, flags, exceptions, stability: None }` (adds SCO-018 for α < .5 or negative, SCO-023 for N < 30; never bootstraps) |

## Spec sections implemented

§12 preamble data rules (pooling in master space, item-set classes, not reached, voids),
§12.1 (difficulty and flags, upper–lower D with ties at the cut, uncorrected point-biserial,
corrected item–rest r with Fisher-z CI and bands, option r, distractor functioning and
trace lines, KR-20, KR-21, α, α-if-deleted, Feldt CI, SEM, Lord's CSEM, summary, zero
variance as n/a, negative α reported with the hint), §12.2 (ranked rules 1–6, multi-select
rule, codes; **flag stability**: N < 1,000 the share of 1,000 bootstrap resamples, N ≥ 1,000
the rule at both ends of the Fisher-z CI; a share not above 50 % beyond Monte Carlo noise or
failing at one CI end → low confidence *(proposed)*, and a low-confidence rank-1 flag is the
non-blocking SCO-002 `proposed_miskey`, never the SCO-001 ERROR), §12.3 (tiers, SCO-023, suppression; the bootstrap as a separate job, the
synchronous path with Fisher-z and Feldt CIs only).

## Flag stability

The job resamples examinees (1,000 draws of N with replacement from SplitMix64 seeded by
`content_seed` — SHA-384 of every item's options, maximum and keyed set and of every cell —
and `"flag_stability"`; never the score-run ID, which carries the key's version number, so the
same responses and key give the same shares whichever key history led there) and recomputes
the flagged items in full: an examinee's number-right total does not change under resampling,
so each resample costs the upper/lower groups (N log N) plus one item pass per flagged item,
with the same arithmetic as `item_stats` (`ctt::item_stat` over a row view), and every other
item's r_ir only (`ctt::r_ir_of`) for its percentile 95 % CI (§12.3; given when r_ir is
defined in ≥ 90 % of the resamples). The CI ends are evaluated at every N on the statistic
that decides the rule: r_ir(key) (rules 1 and 3, the Fisher-z CI the item report shows), the
keyed option's r (multi-select), both options' r (rule 4), and the difference of the
upper-group proportions (rule 2, normal CI). Low confidence = the share's 95 % Monte Carlo
interval (`mc_interval`: ±1.96·√(s(1 − s)/B)) does not lie above 50 % (at B = 1,000 a share
of 52 % is ±3 points) or a failing end.

Oracles (`tests/stability.rs`): a class with a genuine mis-key (an attractive distractor
keyed) gives SCO-001 with a share > 99 % and both ends holding; an item 97 % answer correctly
whose only other answer is the top scorer's gives a rule-1 flag that fails at the upper end
of its CI and fires in < 70 % of resamples → low confidence, labelled in the flag and the
SCO-002 `proposed_miskey` detail; identical reports for the same content under another run
ID; every item's percentile CI (none claimed where r_ir is undefined in > 10 % of the
resamples); N = 1,200 uses the Fisher-z rule. Cost (`tests/scale.rs`, release): N = 300 ×
k = 100 with 6 rank 1–4 flags ≈ 40 ms; every item flagged ≈ 1.2–1.4 s. On the real IAU
exams (gated, `tests/real_iau_stability.rs`): ≈ 30–50 ms at N = 51 and 92.

## Performance

`tests/scale.rs` (release) reports the synchronous path (`analyze`) at 10,000 examinees ×
100 items: about 55–80 ms on this shared 4-vCPU container (from 650 ms before single-pass
option accumulation, covariance-based α-if-deleted and selection-based quintiles). The
spec's estimate is < 10 ms; see open issues.

## Deviations and deferrals

- Flag stability: rule 2 has no correlation, so its "CI" is a normal interval of the
  upper-group proportion difference (the spec names only the Fisher-z CI); the CI ends are
  also reported (and used) below N = 1,000, which makes small classes conservative.
  `analyze` alone leaves `stability` empty; `omr-app` runs the job off the regrade path
  (`ExamProject::flag_stability`, kept per score run) and its exports always carry it
  (`analytics_with_stability`): `item_analysis.csv`, `report.html` and the XLSX Flags sheet show
  each rank 1–4 flag's share and its low-confidence label (F2, adversarial verification).
- Rank-based quintiles break ties by examinee order.
- Deferred: Rasch, DIF, marker severity, standard setting, integrity analytics
  (§12.4–§12.7), reference-implementation CI oracles (§12.8), per-printed-position views,
  a bootstrap CI for α (Feldt only; r_ir has both), labelled diffs across sittings.
