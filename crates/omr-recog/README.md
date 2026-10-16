# omr-recog

The recognition engine (spec §8). This increment implements the **legacy-form path** (§6.10:
printed forms without fiducials) and reproduces, in Rust, the algorithm the Python prototype
validated on 143 real IAU CCSIT answer sheets: ring-lattice registration, per-bubble darkness in
the bitonal degraded mode (§7.2, CAP-003), the batch baseline β_p (§8.6) and the per-question
decision logic (§8.8) with Appendix A exceptions (§13.1). Engine crate: key-blind (R2), no I/O
and no threads (R4), `#![forbid(unsafe_code)]` (R6), deterministic (§8.11).

## Public API outline

```rust
// Engine for one template revision + configuration.
let engine = LegacyEngine::iau_ccsit_v1()?;                         // committed IAU template
let engine = LegacyEngine::new(template, &blank_png_bytes, LegacyConfig::bitonal_v1())?;
engine.stamp(&mut manifest);        // template_sha384 + config_sha384 (EngineManifest, §14.6)
engine.config_sha384(), engine.template_sha384(), engine.prior() /* β_p⁰ */, engine.template()

// Working resolution: integer box factor from the page size (header DPI distrusted, §8.2).
engine.working_factor(w, h); engine.to_working_resolution(&gray)    // 600 dpi bitonal → 200 dpi
// (packed 1-bit data: omr_imaging::reduce_bitmap(&bitmap, engine.working_factor(w, h)))

// Page stage (pure; one task per page in omr-app).
let m: PageMeasurement = engine.measure_page(&gray);   // = register_page + measure_registered
m.registration: RegistrationQuality { candidates, detections, ring_radius_px, pitch_px,
    coarse_ncc, alias_margin, matched, template_points, rms_px, scale, rotation_rad, anisotropy }
m.warp: Option<QuadWarp>, m.registration_exceptions, m.bubbles: Vec<BubbleMeasure {
    centre, snapped, radius_px, inner: Darkness, core: Darkness, unreadable: Option<Unreadable> }>

// Batch stage.
let baseline = engine.batch_baseline(&[&m1, &m2, …]);  // order-independent; MRK-016 prior_model
let baseline = engine.prior_baseline();                // single-page mode: blank-form prior only
let req = PageRequest { n_questions: 29, high_stakes: false, capture: decoded.capture, page: Some(id) };
let r: LegacyPageResult = engine.decide(&m, &baseline, &req);   // or engine.recognize_single(&gray, &req)
r.questions: Vec<omr_model::QuestionOutcome>    // Single/Blank/Multi/Unreadable, auto_accepted, exceptions
r.student_number / r.section / r.date: GridField { read: omr_model::IdGridRead, review: Vec<bool>, exceptions }
    .text() ("2…", '_' confident blank, '?' not confident), .confident()
r.version / r.exam / r.gender: ChoiceRead { labels, response, confident, logit, exceptions } .value()
r.level (page mark level), r.registration, r.exceptions (REG-*, CAP-003 bitonal, MRK-015)

// Building blocks.
decide_group(&deltas, level, &DecisionConfig, high_stakes) -> GroupDecision { kind, accepted, review, … }
page_level(top1s, &cfg); decide::option_logits; decide::decision_logit
LegacyConfig { version, registration, measure, baseline, decision }.sha384_hex() / .canonical_json()
```

`IdGridRead` columns are never auto-accepted (§8.8): per-digit pseudo-logits carry the evidence
(confident ±(L_q + margin), uncertain within (−L_q, L_q)) and `GridField::review` flags the
columns identity must treat as uncertain (§9.5 channel A).

## Pipeline (brief "Algorithm validated" → code)

| Step | Where | What |
|---|---|---|
| 1 Decode, 3 × 3 coverage gray | `omr_imaging::{decode, reduce_bitmap, reduce_area}` | 600 → 200 dpi, `round(255·white/9)` |
| 2 Ring detection | `register.rs` | half-res candidate map (one radius), full-res Gaussian-ring NCC (w 1.6, zero-variance guard), local maxima over 1.6 R, NCC > 0.40, parabolic sub-pixel |
| 3 Pitch | `register::pitch_vector` | densest cluster of near-horizontal NN vectors, 42 ≤ dx ≤ 56 → s, θ; radius = set member nearest 13.5·s |
| 4 Coarse translation | `register::coarse` | NCC with the central 84 % of the blank form: 1/32 full search (upright and 180°), 1/16, 5 × 3 scale/angle hypotheses, 1/8, 1/4; one-pitch alias check |
| 5 ICP | `register_with` | thresholds 12 → 8 → 6 → 4 → 3 → 3 px; LMedS affine × 2; quadratic WLS with 1/max(1, r/1.5); matched/RMS within 4 px |
| 6 Snap | `measure.rs` | detected ring within 3 px, else the predicted centre (filled bubbles) |
| 7 Measure | `measure.rs` | inner r ≤ 0.72 R_local, core r ≤ 0.45 R_local, R_local = 13.5 × local scale; `Darkness` (1/1024) |
| β_p | `baseline.rs` | median of unfilled (< 0.45) instances, ≥ 5 needed, cap 0.35; fallback β_p⁰ + field offset |
| 8 Decide | `decide.rs` | level = median top-1 δ ≥ 0.25, clamp [0.30, 1.0]; table below |

| Outcome | Rule (δ = inner − β_p, n = δ/level) | Exception |
|---|---|---|
| Blank | top-1 δ < 0.08 | MRK-015 ERROR (Booklet) if every question is blank |
| Multi | ≥ 2 options with n ≥ 0.80 and δ ≥ 0.30; others clearly below selection | MRK-002 INFO (auto) / ERROR (`multi_review`, `uncertain_set`) |
| Single | exactly one option with n ≥ 0.55 and δ ≥ 0.20, ρ = top2/top1 ≤ 0.5, top1 − top2 ≥ 0.20 | MRK-003 `resolved_erasure` INFO when the runner-up > 0.10 |
| Review | lone weak mark, near a threshold (hysteresis h), two marks, unresolved residue | MRK-001 `weak_mark` / `hysteresis`, MRK-002, MRK-003 (ERROR) |
| Unreadable | registration failed, disk cut off, < 20 px, no local support — never Blank | REG-003 / CAP-009 / CAP-002 (ERROR, Region) |

Every comparison is on integers (1/1024). Hysteresis: an accepted decision must clear each
threshold by h = 8/1024, doubled for bitonal input (§8.7 step 7). High-stakes mode makes the
erasure rule review-only. Page exceptions: REG-001 BLOCKER (no lattice / no correlation),
REG-002 INFO (upside down, auto-rotated), REG-003 WARN/ERROR (matched < 70 % / < 45 %, RMS
> 0.25 / 0.5 mm, one-pitch alias margin < 0.05), REG-004 WARN/ERROR (scale, skew > 3°,
anisotropy > 2 %), CAP-003 `bitonal` WARN (Batch). All exceptions go through
`omr_model::Exception::must` (Appendix A checked).

## Real-data evaluation (143 IAU sheets; aggregates only)

`OMR_IAU_DIR=<realdata> OMR_EVAL_OUT=<file> [OMR_EVAL_REPEAT=5] cargo run --release -p omr-recog --example eval_iau`;
`tests/real_iau.rs` runs the same evaluation with assertions and skips when `OMR_IAU_DIR` is
unset. Results (pooled batch baseline, h = 16/1024 for the bitonal input):

| Metric | Result | Prototype |
|---|---|---|
| Pages registered | 143 / 143; matched 607–652 of 687 (median 638); RMS 0.28–1.24 px (median 0.45) | ≥ 600; ≤ 1.25 px |
| Auto-accepted answers / silent errors | 4,089 / **0** (SER = 0, 95 % bound 3/n = 7.3 × 10⁻⁴) | 4,091 / 0 |
| Review rate | 7 / 4,096 = **0.17 %** (4 `weak_mark`, 2 `hysteresis`, 1 two-marks) | 0.12 % |
| Student number | 140 exact, 3 flagged, **0 wrong unflagged** | 140 / 3 / 0 |
| Section / version / exam | 142/1/0 · 142/1/0 · 141/2/0 | 142/1 · 142/1 |
| Single-page mode (prior β only) | SER 0, AR 0.17 %, same ID/section/version reads | — |
| Centres vs prototype | median 0.040 px, p95 0.060 px (prototype centres rounded to 0.1 px); snap flag agreement 99.96 % | — |
| Decisions vs prototype | identical read and routing on 4,093 / 4,096 (2 extra hysteresis reviews, 1 review with a different proposal) | — |
| Registration + measurement | **median 27.9 ms, p95 31.6 ms, max 36.2 ms** per page, one thread (release, min of 5 runs on a loaded 4-vCPU container); decide 0.02 ms; PNG decode ≈ 15 ms extra | target ≤ 40 ms |

The alias guard separates cleanly on every page (aligned NCC beats every one-pitch translation
by ≥ 0.166, median 0.25; threshold 0.05).

## Tests

- Unit: decision table on hand-built deltas from the brief's numbers (`decide.rs`), hysteresis
  and high-stakes, page level, baseline fallback for an always-filled position and page-order
  independence (`baseline.rs`), pitch of a lattice, config hash stability.
- `tests/synthetic.rs` (the committed blank form warped by a known quadratic map, marks
  painted at the true centres): warp recovered to < 1 px everywhere; solid fill, blank,
  fill + residue (MRK-003 INFO), multi (MRK-002 INFO), lone stipple on a solid page (MRK-001
  review, never blank), erased letter below baseline, an all-light-pencil page, ID / section /
  version / exam / gender; cut-off scan → `Unreadable` + CAP-009 (`req_a10_…`); blank page →
  REG-001 and all `Unreadable` (`req_a10_…`); determinism (`req_a5_…`); MRK-015; upside-down
  page (REG-002); a simulated 600-dpi bitonal scan → reduction + CAP-003 (`req_a1_…`).
- `tests/real_iau.rs` (`req_a1_real_iau_sheets_zero_silent_errors`): SER = 0, AR ≤ 0.5 %, no
  wrong-unflagged ID/section/version/exam in all three baseline modes.

## Spec sections

§6.10 (legacy forms), §7.2 + CAP-003 (bitonal mode), §8.1 (page stage / batch stage split),
§8.2 (working resolution, bitonal detection), §8.3 (180° orientation, REG-002), §8.4
(registration gates REG-001/003/004; quadratic warp in place of TPS), §8.5–§8.6 (darkness,
inner disk, β_p with fallback and MRK-016), §8.8 (decision table, erasure rule, multi_review,
hysteresis, ID grids never auto-accepted), §8.9 (20 px bubble gate, coverage → UNREADABLE),
§8.11 (integer decisions, config hash, seeded fits), §8.12 rows 1, 2, 7, 9, 12, 22, 28, 29,
§13.1 + Appendix A (MRK-001/002/003/015/016, CAP-002/003/009, REG-001/002/003/004).

## Deviations

- **Classical cascade only.** No batch GMM, logistic model, CNN, conformal sets or LTT (§8.7);
  decisions use the brief's relative thresholds. `QuestionOutcome::logit` is a margin score
  (accepted: L_q + 40 · margin nats; review: 0), not a calibrated log-odds.
- **Registration** follows the brief (ring lattice + blank-form correlation + quadratic warp),
  not tags/DLT/TPS. Changes for speed: candidates come from a half-resolution blurred-circle map
  at one radius; the detection radius is the set member nearest 13.5 × pitch scale (the
  prototype picked it by the top-400 NCC sum; the result matches the brief: 13.5 px on the
  115 full-scale pages, 12.5 px on the 28 reduced copies); the coarse search is coarse-to-fine with the scale/angle hypotheses ranked
  at 1/16 (the prototype searched them at 1/4).
- **Additions** beyond the prototype: one-pitch alias check (REG-003), 180° pages (REG-002),
  a local-support guard (< 2 matched bubbles within 350 px → UNREADABLE, REG-003 Region),
  hysteresis (2 extra reviews on the real data), MRK-002 `uncertain_set`, finer review codes.
- **Baseline fallback** defaults to β_p⁰ (blank-form print coverage) + the field's median
  (batch − prior) offset; the prototype's field median is `BaselineFallback::FieldMedian`.
  Both give the same aggregate results on the real data (`OMR_EVAL_FALLBACK=field_median`).
- **Per-question count** comes from `PageRequest::n_questions` (exam layout, not a key).
- Thresholds marked *initial* in `config.rs` (candidate NCC, min rings, alias margin, matched
  fractions, support radius) need corpus calibration (§23.5); the bitonal thresholds were
  validated on one university's real sheets only.
- Not implemented: the own-form path (tags, page code, Stage A/B/C), illumination B/K, JPEG
  quality, streak masks, cross-out/tick/circle classes, mirrored pages (REG-005), 90° pages.
