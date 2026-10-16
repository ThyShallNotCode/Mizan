# omr-sim

Semi-synthetic sheet generator (spec §23.3) with ground truth by construction. Test and bench
only: never a normal dependency of a shipped crate (R8, enforced by `omr-archtest`). It may do
file I/O and spawn threads (corpus writer).

## Public API outline

```rust
// Marks a student makes (truth by construction).
let rates = MarkRates::standard();                          // ::clean(), ::hard(), or set fields
let script = SheetScript::random(&t, form, serial, Some(student_id), &rates, &mut rng);
let script = SheetScript::from_responses(&t, form, serial, id, &responses, &rates, &mut rng);
script::selection_state(class, cancel_mode, other_selected) // §8.8 S(c): Some(true)=Y, Some(false)=N, None=R
script::expected_outcome(&classes, cancel_mode) -> Expected // Auto(Response) | Review

// Render, mark and capture.
let sim = Simulator::new(&t, &key, 200);                    // caches the static raster per (form, page)
let pages: Vec<SimPage> = sim.simulate(&script, Profile::Standard, &mut rng)?;
// SimPage { image: GrayImage, truth: PageTruth }
sim.clean_page(form, page, serial)?; sim.paint_page(&mut img, &script, page, &mut rng)?;
capture::capture(&img, &CaptureParams::sample(profile, dpi, &mut rng), &mut rng) -> Captured
capture::encode_jpeg(&img, quality, dpi), capture::encode_png(&img)
profile::apply_h(&truth.mm_to_px, x_mm, y_mm) -> (u, v)    // template mm → image px

// Corpus.
let cfg = CorpusConfig::standard(2_000, seed);              // or set: profile, dpi, format, roster, rates, threads
write_corpus(&t, &key, &cfg, out_dir)? -> CorpusSummary     // images/, truth.jsonl, manifest.json
read_truth(path)? -> Vec<PageTruth>
corpus::sheet_script(&t, &cfg, &order, index)               // reproduce any sheet's script + RNG
```

**Truth record** (`truth.jsonl`, one line per page image, ordered by sheet then page):
sheet index, file, page/pages, form, booklet serial, the printed 41-character payload, student
ID and the ID digits bubbled on the page, instrument, profile, all capture parameters, the
`mm_to_px` homography (pixel `(i, j)` covers `[i, i+1)`), and per question: `intended`
(`omr_model::Response`), `expected` (`Auto(Response)` or `Review` under the exam's cancel mode,
§8.8), the class of every bubble (`omr_model::MarkClass`), a difficulty tag
(`Easy | Blank | Light | Erasure | Multi | Ambiguous`) and each painted mark with its target and
achieved interior coverage and mean ink; plus stray strokes.

**Manifest** (`manifest.json`): generator version, profile, seed, dpi, format, template SHA-384,
40-bit exam ID, sheet/page/question counts, mark rates, roster size, first serial. No timestamps:
the corpus is byte-identical for a given seed at any thread count.

## Model

- **Marks** (image space, at the capture resolution, using the layout geometry): `Filled`
  (pencil texture with grain, hatching and coverage holes, 70–100 % coverage, darkness 0.55–0.95,
  wobbly outline with slight overshoot), `LightFill` (darkness 0.25–0.5), `ErasedResidue`
  (smooth smudge 0.05–0.2 next to the final answer), `CheckOrX` (tick or X strokes), `CrossedOut`
  (fill plus an X), `PartialOrDot` (dot or half fill), pen (uniform 0.85–0.98, crisp), stray
  strokes outside bubbles, ID-grid fills. Ink darkens multiplicatively.
- **Capture** (profiles `Clean`, `Standard`, `Hard`): rotation (±0.2°, ±1.5°, ±4°), translation,
  per-axis scale (print and feed), shear, slight perspective, Gaussian blur σ 0.4–1.2 px, noise,
  linear illumination gradient, gamma, paper and toner levels, scan margin with light or dark
  backing, JPEG quality 70–95, optional dirty-glass streak (hard).
- **Determinism:** ChaCha8, one stream per sheet (`set_stream(index + 1)`), roster order from a
  separate stream; no `HashMap` iteration in outputs.

## Measured

- Darkness separation after a standard capture (200 dpi, test `truth_is_consistent…`): filled
  D ≥ 0.42 (mean 0.71), empty D ≤ 0.09 (mean 0.06), light fills ≈ 0.40, residues ≈ 0.17.
- Throughput (release, this 4-vCPU container, shared with other builds): 19 pages/s for 100-item
  sheets at 200 dpi standard JPEG on 4 threads, i.e. 2,000 sheets in about 1.8 min. Per page on
  one thread: page codes overlay 4 ms, marks 9 ms, capture 140 ms, JPEG 45 ms
  (`cargo run --release -p omr-sim --example stage_timing`).
- `cargo run --release -p omr-sim --example gen_corpus -- <dir> [sheets] [clean|standard|hard] [dpi] [threads]`.

## Spec sections implemented

§23.3 stages 1 (render), 3 (procedural marks), 4 (erasures, cross-out then re-mark), 5 (stray
marks), 7 (scan: skew, scale, illumination, noise, blur, JPEG, streaks); §8.8 outcome table as an
independent truth oracle; §2.4/§23.1 standard slice as the `Standard` profile.

## Deviations and deferrals

- Renders directly at the capture resolution instead of 600 dpi plus a print stage; no dot gain,
  dropout hues, paper effects (tint, folds, bleed-through), photocopy or phone stages, bitonal/G4
  output, 180° or wrong-side-first pages.
- Marks are procedural only (no ≥ 10k real exemplar fills); no circled labels or white-out in
  the generated mixes (the painter supports both).
- Sheets are generic (no personalised header text); multi-page booklets put the ID grid on
  page 1 only.
- Synthetic data never certifies a bound (§23.3); sim-to-real checks are out of scope here.
