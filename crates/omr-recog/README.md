# omr-recog

The recognition engine (spec §8), with two paths that share one decision core (β_p baseline,
page level, group decisions, erasure rule, hysteresis, Appendix A exceptions):

- the **legacy-form path** (§6.10: printed forms without fiducials), which reproduces in Rust
  the algorithm the Python prototype validated on 143 real IAU CCSIT answer sheets:
  ring-lattice registration, per-bubble darkness in the bitonal degraded mode (§7.2, CAP-003),
  the batch baseline β_p (§8.6) and the per-question decision logic (§8.8);
- the **own-form path** (`ownform` module, §6.3–§6.4, §8.2–§8.4): the generated A4 sheets with
  AprilTag 36h11 corner tags, row ticks and the QR page code — see
  [Own-form path](#own-form-path) below.

Engine crate: key-blind (R2), no I/O and no threads created (R4), `#![forbid(unsafe_code)]`
(R6), deterministic (§8.11).

# Legacy-form path

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
columns identity must treat as uncertain (§9.5 channel A). The `omr_model` contract holds:
`ColumnState::Single` means a **confident** digit (what `IdGridRead::exact_digits` and rosterless
identity rely on). A digit only proposed for review is reported as `ColumnState::Multi` with the
candidate in `marked` (an ambiguous column), an unconfident blank stays `Blank` (a wildcard), and
a confident double mark is flagged too. `ChoiceRead::confident` is false for several marks
(a version/exam/gender value is one label or none).

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
| Unreadable | registration failed, disk cut off, < 20 px, no local support or a local scale outside 0.80–1.10 — never Blank | REG-003 / CAP-009 / CAP-002 (ERROR, Region) |

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
| Registration + measurement | **median 27.9 ms, p95 31.6 ms, max 36.2 ms** per page, one thread (release, min of 5 runs on a loaded 4-vCPU container); decide 0.02 ms; PNG decode ≈ 15 ms extra (independent review re-run: median 27.1, p95 29.8, max 31.9 ms) | target ≤ 40 ms |
| ID-grid contract | 0 flagged student-number/section grids yield `exact_digits()` (`id_exact_digits_while_flagged`); 9 unconfident single proposals reported as ambiguous columns | — |

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
  page (REG-002); a simulated 600-dpi bitonal scan → reduction + CAP-003 (`req_a1_…`); a
  stipple-only ID digit is never `Single` (no `exact_digits`) and a double version mark is not
  confident.
- Unit (`legacy.rs`): the `IdColumnRead` state contract for confident, proposed and near-blank
  columns.
- `tests/real_iau.rs` (`req_a1_real_iau_sheets_zero_silent_errors`): SER = 0, AR ≤ 0.5 %, no
  wrong-unflagged ID/section/version/exam in all three baseline modes, and no exact digits
  from a flagged ID grid.

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
- **Known gaps** (independent review): `working_factor` is an integer box factor, so a 300-dpi
  scan (ratio 1.5) is not brought to 200 dpi and fails registration (REG-001/004, every bubble
  UNREADABLE, never blank); `multi_review` and `cancel_mode` come from the engine config
  (`Never`, erase-only), not from the exam via `PageRequest`; there is no ring-visibility or
  white-out check (§8.6 F5/F12), so a sticker or white-out over an unfilled bubble reads as
  blank; β_p is a median, so a position where most "unfilled" instances are light marks would
  get an inflated baseline (capped at 0.35); a lone erasure residue that clears θ_sel on a
  light-pencil page is accepted as a light mark (the brief's rule; the §8.7 CNN would separate
  `erased_residue`).
- Not implemented on this path: illumination B/K, JPEG quality, streak masks,
  cross-out/tick/circle classes, mirrored pages (REG-005), 90° pages (the own-form path below
  has tags, mirroring, quarter turns, B/K normalisation, streak masks and tick/cross-out
  review triggers).

# Own-form path

`omr_recog::ownform` reads the university's own sheets generated by `omr-forms` (any
`Template`: questions, options, forms, ID grid, form grid, one or more pages). It reuses the
shared core (`baseline`, `decide_group`, `page_level`, `option_logits`, the legacy result types
`GridField` / `ChoiceRead` and the outcome mapping) and adds tags, Stage A, the page code, row
ticks, gray normalised darkness, streak masks and classical review triggers.

## Public API outline (own forms)

```rust
use omr_recog::ownform::*;

// Engine for one template revision + configuration (hashes for the manifest, §14.6).
let engine = OwnFormEngine::new(template, OwnFormConfig::gray_v1())
    .with_blank_page(PageNo(1), &blank_render, 200.0)?   // β_p⁰ from omr_forms::render::rasterize_static
    .with_detector(Arc::new(KorniaDetector));            // TagDetector trait: fallback detectors plug in here
engine.stamp(&mut manifest); engine.config_sha384(); engine.template_sha384(); engine.prior(page)

// K_exam registry (page-code secrets only; not an answer key, so R2 holds). Debug is redacted.
let mut reg = CodeRegistry::for_template(&template, exam_key);
reg.register(other_exam_id40, other_key, vec![TemplateRevision(1)]);   // mixed stacks, 40-bit collisions

// Page stage (pure; one task per page in omr-app).
let req = OwnPageRequest { device_dpi: Some(200), header_dpi: None, capture: decoded.capture,
                           high_stakes: false, page: Some(page_id) };
let m: OwnPageMeasurement = engine.measure_page(&gray, &req, &reg);
m.route      // ThisTemplate | OtherTemplate{exam_id40, revision} | OtherTagSet(k) | Unidentified
m.page, m.code(), m.auto_accept_allowed
m.tags: TagSearch { tags: Vec<TagHit{id, hamming, corners}>, set, mirrored, all_ids, downscale }
m.registration: Option<StageA { h /*mm→px*/, model: Affine|Homography, tags_used, tags_refined,
    rms_px, rms_um, max_px, mirrored, quarter_turns, skew_rad, px_per_mm, anisotropy, black, white }>
m.dpi: Option<DpiSource::{Device, Header, Unknown}>
m.identity: Option<PageIdentity { outcome: CodeOutcome, code, copies: Vec<CopyRead{copy, status, payload,
    code, revision_known, attempts}>, exceptions }>
m.ticks: Option<TickField { rows, expected, found, rejected, median_um, p95_um, applied }>
m.streaks: Vec<Streak{x, width, delta}>, m.bubbles: Vec<BubbleMeasure>, m.paper, m.features: Vec<BubbleFeatures>
m.exceptions    // REG-001/002/003/004/005, IDN-001/002, COL-004, CAP-005/006

// Batch stage: β_p per template page over the batch slice (order-independent), then decide.
let b: OwnBaseline = engine.batch_baseline(&[&m1, &m2, …]);   // or batch_baseline(&[]) = priors
let r: OwnPageResult = engine.decide(&m, &b);                   // or engine.recognize_single(&gray, &req, &reg)
r.questions: Vec<QuestionOutcome>      // 0-based exam questions of this page
r.student_id: Option<GridField>        // per-column reads + review flags (never auto-accepted);
                                       // same IdColumnRead contract as the legacy path: Single =
                                       // confident digit, never on an unverified or failed page
r.form: Option<ChoiceRead>             // form grid, if printed (never confident with several marks)
r.exceptions, r.level, r.code, r.page, r.route, r.registration
```

Downstream (`omr-app`): decode → `measure_page` per page in parallel → route
`OtherTemplate` pages to that template's engine → `batch_baseline` per batch slice (device,
§8.1) → `decide`. Render each template page once with `omr_forms::render::rasterize_static` at
the capture DPI and pass it to `with_blank_page`: batch mode is robust without it, but
single-page mode needs the printed-label prior. CAP-003 (bitonal) and CAP-006 should be
de-duplicated per batch; `OwnBaseline::exceptions` (MRK-016) belong to the batch.

## Pipeline (own forms)

| Step | Module | What |
|---|---|---|
| Tags (§8.3 1) | `tags` | four 70 mm image-corner windows; `kornia-apriltag` 0.2.0 behind `TagDetector`, quad search decimated so the 6.4 mm square spans ≈ 16 px (×3 at 200 dpi; the spec's L1), decode at native resolution; retry at ±1 decimation; Hamming ≤ 2 |
| Orientation, mirroring (§8.3 2) | `tags`, `stage_a` | the tag set with ≥ 2 distinct corners wound clockwise wins; otherwise the mirrored image is decoded (REG-005 WARN); quarter turns from the fit (REG-002 INFO); tag sets 1–3 (headers, calibration, proofs) are routed away (IDN-002) |
| Corners | `refine` | per edge, along integer columns/rows: 50 % crossing, then ink mass in a 5-px window (exact for a blurred step edge); TLS line fit with one outlier pass; corners = line intersections; two passes (seed-insensitive) — < 0.05 px on synthetic squares |
| Stage A (§8.4) | `stage_a` | tag corners ↔ layout by tag ID (corner order by best cyclic shift against a similarity from the centres); seeded RANSAC homography (1.5 px) + normalised DLT on the inliers, affine vs homography by AICc, one leave-one-out tag pass; a mirrored page is fitted in the mirrored frame and composed (never resampled) |
| Gates | `stage_a` | REG-003 RMS > 0.25 / 0.5 mm; fewer than three tags → REG-003 ERROR (sheet review: no Stage B, see deviations); REG-004 scale 0.88–1.05 / 0.80–1.10 (capture DPI from the device, or a standard header DPI; else `scale_unverifiable`), skew > 3°, anisotropy > 2 % |
| Page code (§6.4, §8.3 3) | `code` | both QR copies cropped where Stage A puts them (+ 4.5 mm), `rxing` 0.9.3, retry on a 2× upsample, un-mirrored if needed; `omr_forms::pagecode` parse + constant-time 96-bit tag under each candidate K_exam; copy table below |
| Ticks (§8.4 Stage C, simplified) | `ticks` | dark-mass centroid of every 1.2 × 0.6 mm tick; neighbour outlier check; a row's bubbles move by the residual interpolated between its two ticks, applied when the median residual > 60 µm or only two tags were found |
| Streaks (§8.6, CAP-006) | `streaks` | columns whose median offset from their ±4 px neighbours is ≥ 12 gray levels (and ≥ 1.5× the side lobes), 1–4 px wide, inside the printed area: masked from every sample |
| Measure (§8.5–§8.6) | `measure` | centre = Stage A (+ tick) prediction; B = median of the 1.2–1.45 r₀ annulus, K = tag black; inner darkness = mean D = (B − I)/(B − K) over r ≤ 0.8 r₀, exact integers → `Darkness`; features: dark fraction (D > 0.5) and annulus ink (D > 0.35); UNREADABLE (CAP-009 / CAP-002), never blank |
| Decide (§8.8) | shared core + `mod.rs` | gray captures: `gray_decision()` (θ_sel = max(0.20, 0.30·level), θ_strong = max(0.30, 0.70·level); other values as the bitonal slice); bitonal captures (CAP-003): `decision_bitonal`, the shared thresholds validated on the 143 real bitonal sheets (θ_sel = max(0.20, 0.55·level)), hysteresis doubled; then review triggers below; pages without a verified code or with a failed gate are proposals only |

| Copies (§6.4) | Outcome | Exception |
|---|---|---|
| all decoded copies MAC-valid and equal | `Verified` | — |
| one MAC-valid, the other fails its MAC / is unreadable | `OneCopyValid` | IDN-001 INFO (`one_copy_invalid` / none) |
| MAC-valid copies differ | `CopiesDisagree` | COL-004 `code_copies_disagree` ERROR (Booklet) |
| valid MAC, unknown revision | `UnknownRevision` | IDN-002 BLOCKER |
| decoded, nothing MAC-valid (forged, foreign, unknown exam, malformed) | `Invalid` | IDN-002 BLOCKER |
| nothing decoded | `NotDecoded` | IDN-001 `unresolved` ERROR |

**Review triggers** (stand-ins for the §8.7 CNN router; they only demote an accepted decision,
the proposal is unchanged): a marked bubble with ≥ 4.5 % annulus ink (a cross-out's strokes)
→ MRK-004; a selected bubble with δ ≤ 0.38, dark fraction 6–45 % and dark fraction − mean D
in [−8 %, +6 %] (a tick or X) → MRK-005; a resolved erasure whose residue δ ≥ 0.171 (as dark
as a light fill, which §8.8 never treats as a residue) → MRK-003 ERROR; `fill_cancels_check`
→ every mark MRK-005. Thresholds are *initial*, chosen on calibration seeds 1 and 3.

## Evaluation (own forms, semi-synthetic)

`OMR_EVAL_OUT=<file> cargo run --release -p omr-recog --example eval_ownform -- [standard] [hard] [seed]`
generates the corpus in memory with `omr-sim` (100 questions × 5 options, 9-digit ID, 2 forms,
200 dpi, JPEG at the sampled quality 70–95, decoded by `omr-imaging`), runs the page stage on 4
threads, the pooled batch baseline and the single-page prior, and compares with the truth.
Held-out seed 20 261 016 (thresholds were chosen on seeds 1 and 3), 500 sheets per profile:

| Metric | Standard | Hard |
|---|---|---|
| Pages registered / code verified / right page | 500 / 500 / 500 | 500 / 500 / 500 |
| Stage A corner RMS (median, max) | 12.8 µm, 21.2 µm | 18.4 µm, 37.8 µm |
| True bubble-centre error (median, p99, max) | 0.049, 0.087, 0.105 px | 0.051, 0.107, 0.346 px |
| Questions / auto-accepted | 50,000 / 49,750 | 50,000 / 47,057 |
| **Silent errors** (pooled batch) | **0** (95 % bound 3/n = 6.0 × 10⁻⁵) | 1 (2.1 × 10⁻⁵) |
| **Review rate** (pooled / single-page prior) | **0.50 % / 0.51 %** | 5.9 % / 5.9 % |
| Ticks or cross-outs auto-accepted (§8.8 R classes) | 0 | 0 |
| Student ID exact / flagged / wrong unflagged | 500 / 0 / 0 | 500 / 0 / 0 |
| Page stage, one thread (median, p95; three runs) | 12.0–13.4 ms, 14.1–18.8 ms | 12.9–14.1 ms, 15.4–19.2 ms |
| JPEG decode (median) | 13.5–15.3 ms | 13.9–14.7 ms |

An independent review re-ran this evaluation after its fixes (bitonal thresholds, two-tag gate,
ID-grid contract): identical counts. Those fixes do not touch this corpus, where every page has
four tags and a gray capture.

Standard reviews: 135 erasures (mostly the residue ceiling), 47 light fills, 25 multis, 35
ambiguous, 8 others. Hard reviews are dominated by the R-class marks the slice adds (ticks,
cross-outs: 2,016 of 3,238 ambiguous questions). The hard silent error is a two-option answer
whose second fill is as faint as an erasure residue (δ < 0.17): the simulator's light-fill and
residue darkness ranges overlap there by construction, and only the deferred class cascade
(`light_fill` vs `erased_residue`) can separate them. Without the review triggers the hard
slice has 510 silent errors on a 300-sheet calibration corpus (cross-outs read as multis); with
the bitonal decision thresholds (θ_sel at 0.55 · level) the standard AR is 2.1 %. The reverse
does not hold. On the 143 real bitonal IAU sheets (legacy measurements, pooled baseline),
`gray_decision()` gives 1 silent error in 4,094 auto-accepted answers. The bitonal thresholds
give 0 in 4,089. Bitonal captures therefore use `decision_bitonal`.
Timings are from a shared, loaded 4-vCPU container (other builds running); the page stage
includes tags, refinement, Stage A, both QR decodes, ticks, streaks and 590 bubbles. The JSON
report (written to `OMR_EVAL_OUT`) also lists per-difficulty counts, review reasons, page
exceptions and feature quantiles per true mark class. No real data is involved.

**Real data.** The 143 IAU sheets are legacy forms (no tags, no code):
`tests/ownform_real_iau.rs` (skips without `OMR_IAU_DIR`) checks that the own-form path
rejects all 143 with REG-001 instead of mis-registering them (median 27 ms per page in the test
profile).

## Tests (own forms)

- Unit (`src/ownform/*`): corner refinement to < 0.05 px on rotated, blurred squares;
  tag-set choice and winding (upright, 180°, mirrored, proof set); detector robustness on odd
  images; copy-outcome table incl. unknown exam and revision (`req_a7_copy_outcomes`);
  similarity / AICc; tick centroid and interpolation; normalised darkness; streak detection
  (dark, light, not content); config hash.
- `tests/ownform_identity.rs`: upright page read exactly; 90°/180°/270° (`req_a7_…`, REG-002);
  mirrored and mirrored + rotated (`req_a7_…`, REG-005); every copy outcome on rendered sheets
  (one copy whitened, one forged, two booklets pasted, forged sheet, unknown exam / revision,
  nothing decodes) (`req_a7_…`); another exam routed, proof sheet routed; blank page REG-001 and
  a scan cut off half way (two tags: REG-003 ERROR, nothing auto-accepted, correct proposals;
  CAP-009 UNREADABLE, never blank) (`req_a10_…`).
- `tests/ownform_gates.rs`:
  - `req_a1_bitonal_…`: on a bitonal capture, a lone residue-strength mark (δ = 0.30 on a
    solid page) goes to review; the gray rule would accept it.
  - `req_a10_two_tag_…`: hard-profile pages with the bottom or top tag pair removed get
    REG-003 ERROR, no auto-accepted question and no exact ID.
  - `req_a6_unverified_…`: an IDN-002 page is never `ColumnState::Single`, has no
    `exact_digits` and no logit reaching +L_q; the proposed digits stay correct.
- `tests/ownform_synth.rs`: 30 standard sheets (SER 0, AR ≤ 1 %, IDs), 12 hard sheets, 6
  two-page booklets (180 questions) routed by page code (`req_a1_…`).
- `tests/ownform_determinism.rs` (`req_a5_…`): byte-identical measurements on 1, 2 and 4
  `rayon` threads (the tag detector's internal loops included) and identical decisions for a
  shuffled batch.
- `tests/ownform_ticks.rs`: a 2.5 px feed-speed bulge (worst bubble 2.43 px off under Stage A
  alone) corrected by the ticks to 0.18 px; decisions correct.
- `tests/ownform_real_iau.rs`: see above.

## Spec sections (own forms)

§6.3 (tag and tick geometry), §6.4 (tag sets, page code fields, 96-bit tag, copy outcomes,
K_exam), §8.2 (measured DPI, decimated tag search, header-DPI rule, `scale_unverifiable`),
§8.3 (tags, orientation, mirroring, page identification, IDN-001/002, COL-004), §8.4 (Stage A
with RANSAC + DLT, AICc, leave-one-out, tag-count rule, gates; row ticks as control points),
§8.5 (D = (B − I)/(B − K)), §8.6 (0.8 r₀ inner disk, β_p, streak masking), §8.7 step 3 (router
triggers as review), §8.8 (shared decisions), §8.9 (contrast CAP-005, streaks CAP-006,
coverage CAP-009, 20 px), §8.11 (seeded fits, integer features, thread-count invariance).

## Deviations (own forms)

- **Stage A only, no Stage B/C proper.** No ECC refinement and no TPS: row ticks give a per-row
  linear correction instead (enough for feed-speed stretch); no Stage D ring search (centres
  are the Stage A/tick prediction). The QR finder centres are not used as extra
  correspondences. No Levenberg–Marquardt after the DLT (the residuals are already ≈ 0.1 px).
- **Two-tag pages are never auto-graded** (REG-003 ERROR, sheet review). §8.4 grades them only
  after a forced Stage B, which is not implemented, plus Stage C. A two-tag affine cannot model
  perspective. In a review probe on 20 pages per row, bubbles ended up to 15 px off on the
  standard slice and 30 px on the hard one. On the hard slice that gave 30 silent errors in
  1,634 auto-accepted answers, even with the row ticks present, because they are then searched
  in the wrong place. Three tags with ticks stay within 1.3 px and are graded.
- **Tag search** uses image-corner windows of 70 mm (spec: 60 mm) and the full-page
  fallback for foreign forms is not implemented (fewer than two tags → REG-001, foreign path).
  Only `kornia-apriltag` is wired; the C `apriltag` fallback plugs into `TagDetector`.
  `kornia-apriltag` uses `rayon` internally on the caller's pool (no pool is created here);
  measurements are still byte-identical across thread counts (tested). Detector and QR decoder
  panics are caught and treated as "not found".
- **Normalised darkness** uses a per-bubble annulus median for B and the tag black for K (spec:
  closing + blur background map at L1, per-tag K interpolation); JPEG-quality estimation and
  its hysteresis doubling (Q̂ < 70) are not implemented.
- **Classical decisions.** As on the legacy path (no GMM, logistic model, CNN, conformal sets,
  LTT); `gray_decision()` lowers the relative selection thresholds for gray captures. The
  review triggers replace the CNN router; under `cross_cancels` a detected cross-out is
  reviewed instead of read as N, and `fill_cancels_check` sheets are always reviewed.
- **Streaks** are detected per page (the spec's "same x on ≥ 3 pages" alert belongs to the
  batch, `omr-app`). The §8.6 rule "more than 30 % of the inner disk masked → `UNREADABLE`
  (CAP-006)" is not implemented: one 1–4 px band covers at most ≈ 15 % of a 200-dpi inner disk,
  so only two bands crossing one bubble could exceed it.
- **Unverified pages** (IDN-001 ERROR, IDN-002, COL-004) are measured provisionally when the
  page is known (single-page template, or agreeing MAC-valid copies) so reviewers see
  proposals; nothing on them is auto-accepted. MRK-015 is raised per page (booklet collation
  is `omr-app`'s).
- **Prior β_p⁰** comes from a blank render supplied by the caller (`with_blank_page`), not from
  the layout's label coverage × 45 % K; without it a constant (60/1024) is used.
- **Shared-core reuse** needed three crate-private visibility changes in `legacy.rs`
  (`question_outcome`, `response_of`, `LegacyEngine::unreadable_exceptions`); no logic changed.
  ID columns use the legacy path's `column_state`, so both paths honour one `IdColumnRead`
  contract.
