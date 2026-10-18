# omr-outcomes

The learning-outcome (CLO/PLO/SO) engine (spec §11). A pure function:
(per-assessment uncurved L3 scores with the §10.1 step-1 artefacts, mapping set, attainment
policy, framework versions) → `AttainmentResult`. Curves, overrides and misconduct
penalties never reach CLO evidence; every comparison against τ and T is exact
(`Ratio<i128>`); only confidence intervals are `f64`.

## Public API outline

| Area | Items |
|---|---|
| Frameworks (§11.1) | `Domain::{Knowledge, Skills, Values}` (K/S/V, TP-154 numbering and headings), `Plo`, `OutcomeFramework::{nqf_ksa, abet_eac_2026, abet_cac_2026(so6), with_extra}`, `Clo` (τ/T overrides, Bloom, version, retired), `CloSet`, `CloPloMap` (strength 1/2/3 = I/P/M), `Crosswalk` (PLO → SO), `Course` |
| Evidence | `AssessmentEvidence::from_run(id, name, W_a, &ScoreRun)` (uncurved L3, s⁺ = max(0, s), voids/excused/unselected excluded), `Offering { sections, roster, assessments, credit_hours }`, `SectionInfo { campus, track }`, `RosterEntry { status, passing }`, `IndirectEvidence` |
| Mappings (§11.2) | `ItemCloMap { weight, note, evidence_uri, rationale, bloom_item, status, source, approved_by, version, … }`, `MappingSet { mode: Full | Split }`, `MappingSet::validate` → SCO-011/012/013/015/032/034, `ItemOutcomeMap` (exit exams → PLO/SO) |
| Policy (§11.4) | `AttainmentPolicy` (all §11.4 parameters and defaults), templates `seu()`, `uom()`, `taif()`, `psu()`, `abet_fcar()`, `abet_generic(τ, T)`, `hash()`, `validate()` (SCO-035), `PolicyPatch` + `cascade`, `mid_term_change` (SCO-025) |
| Formulas (§11.3) | `student_performance` (P_{n,c}: grade-consistent, points-pooled, assessment-equal, custom V), `compute_attainment` → `AttainmentResult { students, clos, exceptions, pooled_blocked, provenance }`, `CloAttainment` (average, threshold-proportion, per-question, levels, EAMU, Wilson/t/bootstrap CIs, verdict, "indistinguishable" badge, direct/indirect blend, groups), `class_stats`, `eamu` (FCAR flags), `pool_proportions`, `mean_of_section_rates`, `suppress_count` |
| Roll-ups (§11.3, §11.5) | `plo_course_level` (strength-weighted, M-only), `plo_student_level`, `mapped_grade_share` (M_{k,p}), `credit_hour_rollup` (AR_p), `exit_exam_attainment` (X_p from ItemOutcomeMap), `exit_exam_blend`, `framework_rollup` (SO_k), `crosswalk_rows` (CLO → PLO → SO), `check_like_scales` (SCO-035) |
| Reports (§11.7) | `tp154_b1` → `B1Table` (rows grouped by NQF domain: number, CLO, related PLOs, generated assessment methods "Midterm Q3–5; Final Q12–20", targeted level, actual level showing average **and** threshold-proportion with the method footnote, comment, comment-required flag, draft watermark); `tp154_a1` → `A1Table` (A+ … F and the status columns, counts and exact %) |

## Spec sections implemented

§11.1 (framework model and templates, one crosswalk for both bodies), §11.2 (ItemCloMap
fields, full vs split, indivisible items, ItemOutcomeMap, section attributes, Bloom
alignment, lint panel codes), §11.3 (all student methods, all class methods except rubric,
Wilson and t intervals, seeded bootstrap, verdict basis, thin evidence, per-student
coverage, small n, direct + indirect blend, like-scales rule, pooling and report groups,
version skew, missing-data table, PLO formulas and the worked example as oracles), §11.4
(policy object, defaults, templates, cascade, versioning, provenance), §11.5 (course,
student and framework roll-ups), §11.7 (TP-154 B.1 and A.1 data, SO crosswalk), §11.8
(edge-case codes).

## Performance

`tests/scale.rs` (release): 10,000 students × 100 items, 6 CLOs, 15 report groups each:
about 70–80 ms on this shared 4-vCPU container (P_{n,c} itself about 35–45 ms, accumulated
in `i128` over common denominators with an exact `Ratio` fallback). The spec's ≤ 50 ms
target for a mapping edit is not yet met here; see open issues.

## Deviations and deferrals

- The rubric class method (PSU) is deferred; the PSU template keeps
  `blend_allowed = false` and decides by threshold-proportion.
- EAMU verdict: "attained" is reported as "no red flag"; the flag itself is in the stats.
- Version skew (SCO-030) withholds the offering-level (pooled) `direct`, `value` and
  `attained` of every CLO (they are `None`, and TP-154 B.1 prints a "pooled result blocked"
  cell); the per-section groups stay available. `stats` still holds the pooled numbers for
  diagnostics only.
- `exit_exam_attainment` computes X_p with the policy's deciding class method (the same
  engine as a CLO, without indirect evidence), so a levels or per-question policy yields a
  level or ρ, never a silently substituted proportion.
- `StudentMethod::Custom` serializes V_{a,c} as a list of `[[assessment, clo], weight]`
  pairs (JSON object keys must be strings), so custom policies hash by content.
- Per-question bootstrap CIs run synchronously only within a budget of 2·10⁶ cell
  operations (`BOOTSTRAP_BUDGET`); larger classes get the CI from the app's background job.
- Not implemented: trends and the Newcombe/Welch change badges (§11.5), closing-the-loop
  ActionItems, evidence sampling and verification packs (§11.6), DOCX filling of the
  official blanks (MVP emits the table data), Hijri labels, retake rules and L5
  `clo_respects_drop` (the flag exists; drop-lowest is not implemented in L5 yet),
  programme lints (§11.5 table), imports of TP-153/TP-151.
- When a roster is given, it defines the student population; evidence rows for students
  not on the roster are ignored (the identity stage reports them as IDN-004).
- ABET SO texts are short summary labels; official wording belongs to the versioned
  framework package.
