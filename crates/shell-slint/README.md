# shell-slint: the OMR Correction desktop shell

Rust + Slint 1.18.1 (spec §5.2, ADR-001). The shell renders and binds; it decides nothing. Every
value on screen comes from a `Backend` as plain-Rust view-models, and every user decision goes
back as a typed call. `AppBackend` implements the trait over `omr-app` (real projects, real
grading); `DemoBackend` serves synthetic data for design work.

```sh
export CARGO_TARGET_DIR=$PWD/target-gui CARGO_INCREMENTAL=0
cargo run -p shell-slint                                     # the last workspace, else ~/OMR Exams
cargo run -p shell-slint -- --workspace ~/OMR/1448-1        # every graded exam below a folder
cargo run -p shell-slint -- --project ~/OMR/1448-1/cs211-midterm-1   # one exam (folder or .omrproj)
cargo run -p shell-slint -- --grade scans/ --template iau-ccsit-v1 --questions 30 \
    --key key.csv --roster roster.csv --clo clo.csv --course "CS 211" --title "Midterm 1" \
    --out ~/OMR/1448-1/cs211-midterm-1                        # grade live on the Scan screen
cargo run -p shell-slint -- --demo --theme dark               # invented demo data, marked DEMO
cargo run -p shell-slint -- --workspace W --screenshot docs/design/screens/real   # every screen
cargo run -p shell-slint -- --workspace W --screenshot D --size 1280x800 --shots review,results
cargo run -p shell-slint --example synthetic_exam -- W         # the synthetic IAU workspace
cargo test -p shell-slint                                      # unit + A11 tests (demo and real)
```

`--grade` takes the definition flags of `omr grade` (`--template`/`--spec`, `--questions`,
`--options`, `--forms`, `--key`/`--infer-key`, `--roster`, `--clo`, `--title`, `--course`,
`--term`, `--cancel-mode`, `--multi-review`, `--id-scheme`, `--threads`); `--out` defaults to a
new folder named after the course and title below `--workspace` (or the current folder).
`--operator` names the reviewer in the audit events (default `$USER`). Every failure prints what
to do (R10), a launch without a display included (run from a desktop session, or use
`--screenshot`).

**First use** (CR-04). Without a source the app opens the workspace used last time (remembered
in `<key dir>/desktop.json`), else `~/OMR Exams`, created owner-only. Demo data appears only
with `--demo` (and for `--screenshot` without a source), and every screen then carries a DEMO
DATA mark in the navigation rail whose label says how to open real exams. On a workspace,
*New exam* (Ctrl+N) opens a form over the dashboard: scans, title, course, sheet
(`iau-ccsit-v1` or an own sheet's spec file), questions, options, forms, and the key, roster
and CLO files. `AppBackend::create_exam` validates it with `omr_app::define` (the rules and
file readers of `omr grade`; a blank key file infers the key), checks that the scans exist,
and starts `run_grading` on the binary's spawner into a new folder of the workspace; the Scan
screen follows the run and the exam opens when it finishes. Setup's *Replace key…* loads a key
file (`ExamProject::replace_key`, a new key version, rescored); the key reads "Key vN · in
force" or "· released", never "draft". Results' *Release results…* asks for confirmation and
releases (`ExamProject::release`), then writes the exports with the Blackboard upload.

`--screenshot` needs no display: it installs a Slint platform with the software renderer at
1440 × 900 (or `--size`) and writes `<dir>/<screen>-<theme>.png` (`-<w>x<h>` is appended for
other sizes). The demo screens are exams, setup, scan, review, review-identity, review-blind,
results, outcomes and settings (`docs/design/screens/`). The real screens
(`docs/design/screens/real/`, from the synthetic workspace) are exams, setup, scan (the
midterm's second batch, with its REG-002 and CAP-012 page notices), scan-add (the Add scans
bar), one review frame per lane with open items (review, review-identity, review-scoring),
review-override (an answer that overrides the suggestion, waiting for its reason), results,
results-export (the Exports bar), outcomes, settings and exams-new (the New-exam form); there is no blind-verify frame because
omr-app draws no blind sample yet. With `--grade` the shots are taken once grading has
finished.

The window size reaches the screens as a copy that `app.slint` writes when the platform
resizes the window (`changed width`), never as a binding: binding `win-w: root.width` closed a
layout loop (the window's constraints depend on the screens, whose layout depends on the width)
that Slint 1.18 reported 99 times as deprecated and liable to panic.

Every screen fits 1440 × 900 nominal and the 1280 × 800 minimum (DESIGN §9). `app.slint` hands
each screen the window size (set by the platform, never by the content, so there is no layout
loop): below 1400 px side columns narrow and the exam drawer overlays the table; below 880 px the
review keyboard legend gives way and long lists scroll. `req_a11_every_screen_fits_nominal_and_minimum_windows`
asserts it.

## Layout

| Path | Role |
|---|---|
| `src/backend.rs` | The `Backend` trait and its view-models: the integration boundary to `omr-app` |
| `src/app_backend.rs` | `AppBackend`: the trait over `omr_app::ExamProject` and `omr_app::vm`; `run_grading`, the body of the grading worker |
| `src/demo.rs` | `DemoBackend`: two fully modelled exams (IAU legacy `iau-ccsit-v1`, 2 versions × 29 questions; a generated 40 × 4 sheet with 4 forms) and six summary-only exams |
| `src/crops.rs` | Synthetic scans drawn with `tiny-skia` for the demo: answer rows, ID grids, page thumbnails |
| `src/keymap.rs` | Review and global keys, mapped to physical positions (Arabic 101 layout, abjad letters, Arabic-Indic digits) |
| `src/bind.rs` | `Shell`: fills the Slint globals from the backend, handles every callback |
| `src/screenshot.rs` | Headless platform, capture and PNG encoding; the demo and real shot lists |
| `src/main.rs` | `omr-desktop`: flags, definition files, the grading worker thread (R4: binaries may) |
| `ui/theme.slint` | Design tokens and components (DESIGN.md) |
| `ui/state.slint` | View-model globals (`App`, `ExamsVM`, `SetupVM`, `ScanVM`, `ReviewVM`, `ResultsVM`, `OutcomesVM`) |
| `ui/widgets.slint` | Shell widgets: switch, segments, editable cells, scan paper, fill bars, queue rows |
| `ui/screens/*.slint` | One file per screen |
| `tests/req_a11.rs` | A11 acceptance tests on the demo backend |
| `tests/req_a11_real.rs` | A11 acceptance tests on the real core (synthetic exam) |
| `tests/support/` | Shared label-driving helpers; `synthetic_exam.rs`, the synthetic IAU exam |
| `examples/synthetic_exam.rs` | Writes the synthetic workspace the real screenshots use |

## The real backend (`AppBackend`)

```rust
let backend = AppBackend::workspace(dir, operator);          // vm::list_exams; lazy open
let backend = AppBackend::project(dir, operator)?;           // one exam, opened read-write
// grading: the binary owns the worker thread (R4), the backend only drains its channel
let (tx, rx) = std::sync::mpsc::channel();
std::thread::spawn(move || run_grading(GradeRequest { definition, inputs, options }, tx));
let mut backend = AppBackend::grading(&definition, rx, workspace, operator);
// adding scans later: the binary hands over a spawner (R4), the backend starts run_append on it
backend.set_spawner(Box::new(|work| std::thread::Builder::new().spawn(work).map(|_| ()).map_err(|e| e.to_string())));
let shell = Shell::bind(&ui, Rc::new(RefCell::new(backend)));
shell.start_live_updates();                                  // Shell::pump about twice a second
```

**Adding scans** (§8.1, `ExamProject::add_scans`): the Scan screen's *Add scans…* opens a bar
(floating, so it never pushes the status bar out of a 1280 × 800 window) with a path field
(folders or files, `;` between several, `~/` for home). *Add and regrade* saves and closes the
project on the UI thread and starts `run_append` on the binary's thread: every stored page is
graded again with the new ones, review decisions carry over, a disagreeing reread is SYS-032.
While it runs the exam is busy: its screens show the live batch (tracker), commands are refused
with what to wait for, the review queue says it is being graded, the dashboard row reads
*Scanning*. When the worker reports, the project reopens (also after a refused batch), the
activity log gains the report line (`batch-2 · 4 files · … · 1 decision kept`) and the shell
drops its review selection (item IDs are new: `Backend::take_reloaded`).

**Page notices**: pages read turned (REG-002: portrait scans of the landscape form, pages fed
upside down) and blank backs dropped (CAP-012) never reach the queue; the finished batch lists
them in the activity log (`REG-002 0047 · portrait, read turned 90° ccw`) and counts them in the
reconciliation card (lines for CAP-011, CAP-012 and REG-002 appear only when non-zero). The
sheet pills count sheets (each once, by its worst open item), not exception items.

**Exports** (§16.3): Results' *Export…* bar writes every export next to the project
(`ExamProject::export`, whose message names the TP-154 files and the Blackboard upload) or only
the Blackboard grade-center upload (`ExamProject::export_blackboard`), into a new column or an
existing one by its column ID. The button is disabled, with the core's reason, for an inferred
key or a rosterless exam (§16.1).

Each exam opens read-write on first use (`ExamProject::open`). Only when another process holds
its store does it fall back to read-only, and every command then says how to free it; a project
that fails its audit checks (SYS-011: an edited snapshot or log, an older copy) is not opened,
and its screens show the reason and what to do.
Derived view-models (review items with their crops, results, outcomes) are cached until the next
command; page thumbnails are decoded once from the stored originals.

| Screen | Source |
|---|---|
| Exams | `vm::list_exams` rows; open exams re-summarised live (`summary_vm`); stats from the session snapshots (sheets graded this week and today, throughput of the latest run) |
| Setup | `setup_vm` (template, keys, CLOs, mapping, lints); edits through `set_key`, `set_mapping_weight`, `set_mapping_note` |
| Scan | live: `vm::ProgressTracker` over the worker's `Progress` events; finished: the session (timings, pages, open items by code, batch exceptions, reconciliation counts, real page thumbnails) |
| Review | `review_queue_vm` / `review_item_vm`: real crops and overlays, engine suggestion with its fill and margin, group, notice evidence; identity candidates joined with the roster; form items as the version bubbles |
| Results | `results_vm` plus `analytics()` (α, SEM, item flags with codes, rank and stability, keyed option) |
| Outcomes | `outcomes()` (μ, π and their CIs, verdict, policy and version) and `outcomes_vm` (TP-154 B.1 cells) |

Decisions map one to one: options are `Decision::Option(n)`; on identity items a candidate `n`
is `Decision::Option(n)` (the n-th listed candidate), N is `NotOnRoster`, a typed student number
(the identity frame's *Student number* field, or digits in Roster search) is
`ReviewAction::SetStudent` → `Decision::Student` (on the roster when there is one; any number
without a roster, so a rosterless exam's identities can be settled); `M` without options is
`Decision::Multi(vec![])` (the core takes the marked options); version bubbles map to the
exam's form with that printed letter (`Decision::Form`). An answer that overrides the engine's
suggestion needs a reason code (§13.4): `Backend::needs_reason` asks the core
(`overrides_engine`), the frame offers the item family's first four codes and Other, and a last
chip lists every canonical code the core offers (`reasons_for`, §14.4); the commit waits for one
(`apply_with_reason`, the note travels with it). A reason that needs a note (Other,
SOLO_SELF_APPROVAL: `ReviewItem::reasons_needing_note`) keeps Accept on *Write why in the note*
until there is one. The note field is two-way bound, so a note typed and followed directly by
Accept (mouse, key or AT) is never lost to a deferred focus-out commit. `D` (duplicate) has no
counterpart in omr-app and is refused with the alternative. An identity candidate with a
posterior below 0.5 (the nearest roster student of a number that is not on the roster) is
listed, never offered as the one-key suggestion.

Nothing the core lacks is shown as if it were real: item statistics the core did not compute
(p, r, its CI and band read "–" / "not computed", never 0), a roster (none: "no roster:
absences are unknown", the Setup step is a warning), mapping approval (rows read "in use", the
drawer "mapped", the outcomes footnote names every mapping of the definition), review times (median shows "not recorded"),
the write-in boxes and seat-map identity channels (left out), national IDs (left out), a
separate unregistered crop (F2 has nothing to switch to; the caption says "scan"), L6 status
counts (none), two-person rule (off), Hijri dates (empty). Thumbnails without a page image
are blank sheets. The What-if, Stop, Queue filter and Rescan pull list buttons say that they are
not built yet and name the command that does it (`omr review list --lane capture`, …); New exam
on demo data says how to open real exams; the TP-154 button writes the exam's exports
(`ExamProject::export`) next to its project.

## Contract highlights

The doc comments in `backend.rs` are authoritative:
- queries are `&self` snapshots; commands return `Result<_, BackendError>` whose message the
  shell shows as is;
- lists arrive in display order (queue order is the core's, §13.2);
- display strings the core owns arrive formatted (term labels, Hijri dates, TP-154 cells);
- review view-models never carry key-derived values (§13.2, §19.3); blind items have no
  suggestion, fills or overlays (§13.4);
- blind items carry bubble geometry only (no fill, no state); the shell masks them again so a
  backend slip cannot leak the engine's reading;
- `apply` returns the next item in queue order; `undo` compensates the latest event and returns
  the item it re-opened;
- crops are RGBA8 (`Arc<[u8]>`) with bubble geometry in crop pixels; small crops at scan
  resolution are enlarged on screen to about 600 × 260 px at 3×;
- `ReadinessCheck::target` says where a readiness action leads (review lane, scan, setup,
  outcomes); setup lints use Appendix A codes (SCO-011 split weights, SCO-013 unmapped item,
  SCO-014 thin evidence, SCO-032 unapproved mapping);
- item-analysis rows may arrive in any order: the shell lists flagged items first (worst flag,
  stable before "proposed", then question).

## Synthetic exam and evidence

`tests/support/synthetic_exam.rs` paints pencil marks (omr-sim's mark painter) on the committed
blank IAU form, adds a light scanner model and grades through omr-app: CS 211 Midterm 1,
60 sheets, versions A and B, 30 questions, a 62-student roster with invented English and Arabic
names (IDs start with 9), four CLOs with split weights and an Arabic note. The mix gives 14 open
items: 10 light marks (MRK-001), a double mark (MRK-002, `multi_review = always`), a blank
version bubble (SCO-005, form inferred), a number not on the roster and a faint ID digit
(IDN-004), plus a mis-key on form A Q12 that item analysis flags. A second exam (CS 212 Quiz 2,
24 sheets) is graded with an inferred key. No real scan, name or student number is used.

| Test | What it proves |
|---|---|
| `req_a11_real_backend_review_flow_rescores_results` | the synthetic midterm is graded, opened with `AppBackend` and reviewed through accessible labels: accept a suggestion, choose the keyed answer (an override: the reason chip first), mark a blank, undo it, resolve an identity as not on roster, accept an inferred form; each step's booklet percentage and the class mean move exactly as the core rescored them, the reason is in the audit record, and the results view-model equals a full rebuild from the saved project; every remaining item settled, then *Release results…* and *Release* release the exam (the Blackboard upload is written); the Exports bar writes the Blackboard upload into column 835932 (usernames and scores only, no names) and every export |
| `req_a11_real_backend_grades_live_through_pump` | a background grading run (the test owns the worker) drives the Scan screen through `pump`; empty states before the project exists; afterwards the batch is complete with real thumbnails and results |
| `req_a11_real_backend_adds_scans_from_the_scan_screen` | day one graded, one decision taken, then *Add scans…* with day two (a portrait scan and a blank back) through accessible labels: while the worker runs the exam is busy (commands refused, queue says why, row *Scanning*); afterwards batch 2 of 2 with 8 booklets, the REG-002 and CAP-012 notices, the decision kept |
| `req_a11_real_backend_edge_projects_never_panic` | a project without roster or CLOs, graded with an inferred key: every screen and key, Blackboard disabled with the reason, no invented roster; an override with Other and its note; a typed student number; then every item settled and the empty queue (undo re-opens) |
| `req_a11_real_backend_empty_workspace_never_panics` | an empty workspace: every screen and key, empty states |
| `req_a11_real_backend_replaces_the_key_from_setup_and_names_a_tampered_project` | an inferred key replaced from Setup (*Replace key…*, the key file, *Replace and rescore*): the key is in force and no longer INFERRED; a snapshot edited outside the app is refused with SYS-011 and the remedy, never "close the other program" |
| `req_a11_real_backend_creates_an_exam_from_the_new_exam_form` | *New exam* on a workspace: a question count that is not a number and a missing key file are refused with what to type, nothing started; then the scans graded with an inferred key into a new folder of the workspace, followed on the Scan screen, and the exam opens |
| `req_a11_demo_data_is_marked_on_every_screen` (demo file) | the DEMO DATA mark on every screen, release refused on demo data, and New exam saying how to open real exams |
| `req_a11_blind_items_never_show_a_leaked_engine_reading` (demo file) | a backend that sends the engine's reading with blind items: the frame, the sparkline and Enter never show or commit it |
| `req_a11.rs` (demo) | the review flow, keyboard paths, labels, layouts and screens on the demo backend |

On the shared 4-vCPU container the two real tests take about 10 s together (test profile;
rendering and grading 68 sheets included). The synthetic workspace graded at 47–81 pages/s with
the grading crates optimised (`--config profile.dev.package.<crate>.opt-level=3`).

## Accessibility and agent operability (DESIGN §8, §10; ADR-001 S7)

Every interactive control is one AT node with a unique label; inner texts and keycaps are hidden.
Screens are instantiated only while visible, so labels stay unique per screen. The tests drive
the review flow through those labels (`find_by_accessible_label`, `invoke_accessible_default_action`,
`set_accessible_value`) and through key events, and check label uniqueness on every screen in
both themes.

Single-key shortcuts act only while the review canvas has focus and can be switched off in
Settings (WCAG 2.1.4). Every keycap shown on a button works: Ctrl+1…6 and Ctrl+, everywhere;
Ctrl+K and Ctrl+N on Exams (whose table is one tab stop: ↑/↓ move, Enter opens the drawer, Esc
closes it); Ctrl+Space and Ctrl+R on Scan; Ctrl+P on Results. A page change hands focus back
to the window's key scope (Review focuses its canvas). Slint 1.18 reports produced text, not scan codes, so physical positions
are recovered from the Arabic (101) layout (ADR-001 S4 remains open for true scan codes).

## Rules

- R1: only this crate depends on Slint. R8: `i-slint-backend-testing` is a dev-dependency
  (pinned `=1.18.1`, no features).
- R6: `#![deny(unsafe_code)]`; the only `allow(unsafe_code)` sits on the private
  `mod generated { slint::include_modules!(); }`, re-exported as `shell_slint::ui`.
- R4: the library creates no thread, and its only file I/O is writing screenshot PNGs;
  `omr-desktop` owns the worker threads (the spawner) and its settings file; omr-app does the
  rest (definition, key, roster and CLO files through `omr_app::define`, stores, originals,
  exports, its rayon pool).
- No real student data in the demo, the synthetic exam, the tests or the screenshots: invented
  names, 10-digit IDs starting with 9, procedurally drawn scans or painted marks on the blank
  form.
