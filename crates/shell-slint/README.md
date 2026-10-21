# shell-slint: the Mizan desktop shell

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
cargo run -p shell-slint -- --workspace W --screenshot D --size 1280x800 --shots setup-keysheet
cargo run -p shell-slint -- --workspace W --screenshot D --remark W/remark-cs211-midterm-1 \
    --remark-setup W/remark-cs212-quiz-2 \
    --new-exam "W/new-cs213-quiz-1/CS213 Quiz 1"              # + the Remark screens and the card
cargo run -p shell-slint -- --grade "W/new-cs213-quiz-1/CS213 Quiz 1" --template iau-ccsit-v1 \
    --questions 30 --key W/new-cs213-quiz-1/key.csv --workspace W --screenshot D \
    --shots scan-notice                                        # the completion notice (grades W)
cargo run -p shell-slint -- --workspace EMPTY --screenshot D   # an empty exams folder
cargo test -p shell-slint                                      # unit + A11 tests (demo and real)
```

`--grade` takes the definition flags of `omr grade` (`--template`/`--spec`, `--questions`,
`--options`, `--forms`, `--key`/`--infer-key`, `--roster`, `--clo`, `--title`, `--course`,
`--term`, `--cancel-mode`, `--multi-review`, `--id-scheme`, `--threads`); `--out` defaults to a
new folder named after the course and title below `--workspace` (or the current folder).
`--operator` names the reviewer in the audit events (default: the name saved in Settings, else
`$USER`, else `USERNAME`), validated by `omr_app::app::operator_principal`. Every failure prints what
to do (R10), a launch without a display included (run from a desktop session, or use
`--screenshot`).

**First use** (CR-04). Without a source the app opens the workspace used last time (remembered
in the preferences, `<key dir>/desktop.json`, owner-only), else `~/OMR Exams`, created
owner-only. Demo data appears only
with `--demo` (and for `--screenshot` without a source), and every screen then carries a DEMO
DATA mark in the navigation rail whose label says how to open real exams. On a workspace,
*New exam* (Ctrl+N) opens the New-exam card over the dashboard (G-3). Left, what the
instructor gives: the scans, the key file or key sheets, the roster and CLO files, and any
value typed over the proposed one (title, course, questions, versions, options; *Advanced
options*, whose fields exist only once opened: sheet, output folder, term, student-ID scheme,
cancel mode, locked when the sheet prints it, second review, chosen from its three choices,
high stakes; an output folder typed as a name is a folder in the exams folder). Right, what is graded:
every value as `omr_app::propose` proposes it, "value — source (evidence)", the questions
that stop grading with their one-click answers, and the notes, folded. The shell decides
nothing: `Backend::propose_new_exam` takes the form, starts `run_survey` on the binary's
spawner when the scans, sheet, key sheets or roster differ from those surveyed (stopping the
one running; each survey has a generation and a report of another is dropped) and returns the
`NewExamCard`; the survey's progress and end arrive through `Backend::pump`. A field edit
proposes again after a 300 ms pause (`CARD_DEBOUNCE`), a dialog's pick at once; a choice goes
back as `choose_on_card` (an answer is recorded; a file to pick, a field to empty or to type
in is the window's to do); an answer holds while its field keeps the value it was given for
(`Given::drop_answers_for_retyped`) and while the scans it was given for stay; *Stop checking* is `stop_survey`. *Grade exam* is enabled only when
the card has no blocker (`NewExamCard::blocker`: another exam being graded, the scans being
checked or not checked, an open question); `create_exam` grades the proposal's `Inputs`
exactly (with each value's provenance) into the proposal's folder, and refuses a form changed
since the card was shown (the card is proposed again first). The demo shows an invented card.
The Scan screen follows the run and the exam opens when it finishes. Each worker (a new exam,
added scans, a roster) writes the exports when its run ends, as `omr grade` does
(`GradeRequest`/`AppendRequest`/`RosterRequest::export_to`, from Settings' *Write exports after
grading*, `Backend::set_auto_export`; a key read from key sheets holds them until it is
confirmed), and reports how it ended once (`Backend::take_completed`, a `JobDone`): a notice
sits above the status bar of every screen, in room each screen gives up for it
(`App.notice-room`), with what happened, what waits and the next steps the backend names
(`NextStep`: review, results, the exports folder, Setup's key confirmation or *Replace key*,
the card again, the Scan log). It is a live region (assertive when the run failed), dismissed
with ×, Escape, a step that leads to another screen, New exam or grading another exam; the
keyboard goes back to the window as it goes. Once a decision or a key change follows, the
notice and Results' *Export…* say its exports are older (`Backend::exports_state`, from
`omr_app::export::exports_state`). Review opens on a lane with open items. A new exam being graded is cancelled from the Scan screen (*Cancel grading…*,
then the confirmation; `cancel_job` sets the run's `omr_app::Cancel`): nothing is saved, no
folder is left, and *Edit and grade again* refills the card, the answers given on it included
(`Backend::restore_graded_card`). Once the run has read its pages
it is being saved (`Backend::saving`, the token committed): the cancel is refused as too late
and says so, and *Cancel grading…* goes. Closing the window while a run is in progress asks
first (`Shell::close_requested`, wired to the window's close request): kept, it runs on;
cancelled, the window closes once the run has stopped, and asked again meanwhile it still
waits; a run being saved is kept, and the window offers to close once it is saved. The
dialog is modal for the keyboard too: Escape answers *Keep it running*, Tab stays on its two
buttons and no shortcut reaches the screen underneath. Should the
event loop end while a run goes on, the binary cancels it and waits (`backend::wind_down`),
so no worker is killed half-way. Setup's *Add roster…* (while the exam has none) and step *4
Roster* open the roster bar: `set_roster` reads and checks the file on the UI thread
(`define::read_roster_path`, a dry `import_roster`), then `run_set_roster` identifies every
booklet again on the spawner (`ExamProject::set_roster`, decisions carried over) as a run the
Scan screen follows. Every path field (`PathField`) has
*Browse…* beside it, and *Folder…* for scans, when the window has file dialogs: the binary hands
the shell a `host::Host` (`Shell::set_host`) with native dialogs (`rfd`, parented to the
window), the file manager and the preferences; the library only describes each dialog
(`PickRequest`: kind, title, filters, the folder it opens in). Paths a dialog picked are used
exactly while the field still shows them (a `;` in a file name stays intact); an edited field
is read as typed. Headless tests have no host, so no Browse button; screenshots show the
window as the app has it, with a host whose dialogs never open (`screenshot::display_host`).
A path the window proposes (the card's folder, the parallel run's report folder) or writes in
the Scan log reads `~` for the home folder, as the status bar does (`bind::home_path`).
*Open folder…* (Ctrl+O; *Change…* in Settings) picks another exams folder:
`Backend::open_workspace` saves and closes every exam open (`close_all`, which the binary also
calls when the window closes, so each writes its signed checkpoint) and lists the new folder; a
folder holding one exam opens the folder above it with that exam selected
(`open_exam_folder`). It is refused while an exam is being graded. *Refresh* (F5) re-lists the
folder (`refresh_workspace`); an exam open here stays even if its folder is gone. The drawer's
*Open exam folder*, Results' *Open exports folder* and Settings' folder button show a folder
the backend names (`exam_folder`, `workspace_dir`) in the file manager, never a typed path or a
file. Settings records the operator's name (`set_operator`: validated by omr-app, set on every
open project), and every setting (the name, the theme, reduced motion, one-key review, the
blank-bubble overlays, the exams folder) is saved in the preferences and applied at the next
start (`--theme` wins over the saved theme). With `--project`, the exams folder is the folder
above the exam's, so a new exam is graded beside it.
Setup's *Replace key…* loads a key
file (`ExamProject::replace_key`, a new key version, rescored); the key reads "Key vN · in
force" or "· released", never "draft". Results' *Release results…* asks for confirmation and
releases (`ExamProject::release`), then writes the exports with the Blackboard upload.

`--screenshot` needs no display: it installs a Slint platform with the software renderer at
1440 × 900 (or `--size`) and writes `<dir>/<screen>-<theme>.png` (`-<w>x<h>` is appended for
other sizes). The demo screens are exams, setup, scan, review, review-identity, review-blind,
results, outcomes, settings and exams-new (the New-exam card, invented) (`docs/design/screens/`). The real screens
(`docs/design/screens/real/`, from the synthetic workspace) are exams, setup, scan (the
midterm's second batch, with its REG-002 and CAP-012 page notices), scan-add (the Add scans
bar), one review frame per lane with open items (review, review-identity, review-scoring),
review-override (an answer that overrides the suggestion, waiting for its reason), results,
results-export (the Exports bar), outcomes, settings, exams-new (the New-exam card as it
opens), and, when the workspace holds an exam graded with key sheets, setup-keysheet (its key
summary to confirm) and scan-keysheet (its batch, the key sheets listed as such). With `--remark
<workbooks or their folder>`: the Remark migration, exams-remark (the New-exam card's import for
review) and results-remark (the parallel run) of the default exam, and setup-remark (Setup's
import, of `--remark-setup` when given, shown for the first exam it applies to). With
`--new-exam <scans folder>` (`key.csv`, `roster.csv` and `clo.csv` beside it; the example writes
`W/new-cs213-quiz-1/`): the New-exam card made from them, its scans checked, exams-new-ask (the
scans alone: the answer key asked, Grade exam waiting) and exams-new-ready (with the files:
every value with its source, the title and course from the scans' folder name, nothing asked).
With `--grade` the shots are taken once grading has finished, and scan-notice adds the
completion notice of that run over the Scan screen (it grades into `--workspace`: use a copy).
An exams folder with no exam gives one shot, exams-empty (its empty state). There is no
blind-verify frame because omr-app draws no blind sample yet. Background work (flag stability,
thumbnails, the card's check of the scans) reports before each capture (`Backend::settle`), so
no shot catches "computing…" or "Checking the scans".

The window size reaches the screens as a copy that `app.slint` writes when the platform
resizes the window (`changed width`), never as a binding: binding `win-w: root.width` closed a
layout loop (the window's constraints depend on the screens, whose layout depends on the width)
that Slint 1.18 reported 99 times as deprecated and liable to panic.

Every screen fits 1440 × 900 nominal and the 1280 × 800 minimum (DESIGN §9). `app.slint` hands
each screen the window size (set by the platform, never by the content, so there is no layout
loop): below 1400 px side columns narrow, the exam drawer overlays the table and the setup
stepper shows step numbers only (the labels keep the names); below 880 px the
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

**Key from answer sheets** (§10.1, CR-11). The instructor's filled answer sheet can be the key;
it is read by the same key-blind recognizer and is **never** accepted on its own:
- *An exam graded with its key sheets* (`omr grade --key-sheet`, or found in the stack by the
  marker rule) is held (SCO-003 BLOCKER `key_sheet_unconfirmed`): nothing is scored. Setup opens
  on its key summary; Results says "Results wait for the key" and leads there; Scan lists the
  hold with the codes. The key tag reads the core's "Key v1 · from key sheets · NOT CONFIRMED"
  until the confirmation.
- *Read key from answer sheet…* (the answer-key card in Setup) opens key-sheet files for an exam
  that already has a key (an inferred one, say): `omr_app::keysheet::read_key_sheets` reads them
  in memory with the exam's recognizer and the summary is shown labelled "Read from … · not
  confirmed: the exam's key is unchanged until you confirm"; *Put aside* drops them.
- The summary shows one version at a time (segments "Version A · 3 to decide"): each question's
  bubble row as scanned, straightened along its own axis (`rectified_row`), with the marks
  outlined in the paper colours of DESIGN §4 (drawn into the image, `crops::with_marks`, so a
  row is one image and one AT node), what the sheet reads (`B+E?`), the key entry it gives and
  its status. Rows that wait for the instructor (several marks SCO-027, no mark, an option not
  used, not read clearly, two sheets that disagree) are highlighted (tint and an inline-start
  bar) with what to do, and offer the options, *Accept A|C* on a several-marks row, and *Void*;
  decisions are taken there and sent with the confirmation.
- *Confirm version A* (or *Confirm every version*) is enabled once every highlighted row is
  decided. For the exam's own key sheets it calls `ExamProject::confirm_key_sheets`: the
  confirmation is recorded with the operator and the scans' SHA-384 (`KeySheetVersionConfirmed`,
  then `KeyVersionPublished { source: key_sheet }` with the last version), the hold lifts and the
  exam is scored. For files read in Setup the in-memory reading is confirmed the same way, then
  the confirmed key replaces the exam's (`replace_key`, reason `ADJUDICATION` with a note naming
  the key-sheet files, pages and SHA-384 prefixes and the operator); a version without a key
  sheet keeps the exam's key, unless that key is inferred (refused: read that version's sheet
  too, or replace the key from a file). A sheet whose grid shows no key-sheet marker (a student's
  sheet, or one registered a row off reading `1111111111`) gives no key and says why.
- Scan lists key sheets as key sheets, never as students: gold-framed thumbnails labelled
  `KEY A` (a refused one in error tone), a *Key sheets N* pill, a reconciliation line
  "Key sheets, not students (§10.1)" (booklets count students only), and an activity line per
  sheet (`KEY cf13·1 · key sheet version A · named · key waits for your confirmation in Setup`);
  a key-sheet thumbnail opens the summary.

**Remark migration** (spec §3.5, PAR-09; parallel run §23.7). Only Remark Office OMR's
documented graded Excel export is read, through omr-app (`remark::read_workbook_files`,
`remark::import`); its `.rmx`/`.aky` files are refused unopened (§3.5: never reverse engineered).
- *Import from Remark export…* exists on the New-exam card (a panel over the card) and in
  Setup's top bar (a bar under it). The workbooks (`;` between several) and an optional points
  file are read and the import is **shown for review, nothing changes**: per version the points'
  source, total and values, "N of N rows reproduced exactly" (every Remark total, and every
  learning-objective point, or the import is refused), the key **as counts** ("30 entries · 0
  with several answers · 0 missing") until *Reveal key*, and per question its exact points and
  learning objective; the learning objectives (each a CLO with weight-1 mappings tagged
  "imported, needs review", proposed); the key sheets Remark graded as students, **excluded**
  (IDs masked); the workbooks. Versions whose objectives differ show their own points (segments
  "Version A · 25 pts").
- A **refusal** replaces the summary with the core's reason (its CLI flag named in this window's
  words) and *What to do* (R10). When the totals leave questions open and a fully determined
  version leaves one value for them, or a learning objective's questions are not told apart by
  the students (a small class: its equal split is never assumed, F4), the suggestion is shown
  and *Use the suggested points* gives it as the instructor's explicit choice
  (`remark::suggest_points`; the summary then reads "Points given: given by you as
  suggested…"); every total is checked again.
- New exam: *Use this import* makes the import the card's key source and clears the key file
  and key sheets (and the CLO file when the export has objectives); a strip says where the key
  comes from (*Clear the Remark import* undoes it). The proposal takes the key, the versions
  and the question count from it (source "Remark import"), and `define` applies it as omr-app
  documents it (`KeySource::Provided` with its keys, `set_key_points` for the exact points per
  version, `outcome_setup()` for the CLO rows "imported, needs review"); a key file or key
  sheets beside it, another question count or other versions are questions on the card. The
  activity log counts the import (no student value).
- Setup: *Apply key and points* replaces the exam's key (`ExamProject::replace_key_points`,
  reason `ADJUDICATION` with a note naming the workbooks' SHA-384 prefixes, the points the
  instructor gave and the operator): a new key version, the exam rescored (an inferred key is no
  longer INFERRED). A graded exam's points are fixed: its form map (master items with their
  points) is stored once and omr-store refuses another, so an export whose points differ is shown
  as **not applicable** with the first differing questions and what to do (create the exam from
  the import, or compare); the points change only for an exam not scored yet (held for its key
  sheets). An export of another question count or other versions, or one none of whose student
  numbers is this exam's (a Remark export carries no exam identity, only its rows: it is another
  exam's or another section's), is shown with why it does not apply. The learning objectives are
  listed for review here; a graded exam's CLOs are changed in the CLO card below the matrix:
  *Approve imported mappings* (shown while imported rows wait: `ExamProject::approve_mappings`
  with the signed-in principal as approver, an audited `MappingApproved`; the TP-154 tables leave
  the draft watermark) and *Replace CLOs* with a CLO file (`replace_outcomes`, audited, the
  outcomes recompute, nothing is graded again). A grid cell that stands for one version's rows
  only is labelled "version A only: …"; the grid's points column shows version A's points.
- Results: *Compare with Remark export…* (a floating bar, Restricted note) runs
  `ExamProject::compare_remark` with crops and shows the parallel run **in place of the results**
  under the Restricted / Q-08 banner (`parallel::RESTRICTED_BANNER`, warning-soft with a lock
  glyph): tiles (booklets paired by ID / partial ID / answers, questions equal, to adjudicate,
  totals equal: **the exam's own grades** against Remark's, with "the exam's maximum is not
  Remark's" when its points differ, identity differences, key sheets), every difference with its
  rectified
  bubble-row crop (settled differences first, with an inline-start bar, then pending ones that
  differ, then pending ones Remark read the same), identity differences and totals that are not
  Remark's (student numbers masked on screen), the key sheets' effect on Remark's reported
  statistics, the workbooks compared or of another section, and *Save report* into a folder
  (default `<exam folder>/parallel-run-remark`): the comparison is rerun on the exam as it stands
  and `export_parallel_run` writes the Restricted report (signed manifest, owner-only, sync
  folders refused, `ExportCreated`). Remark's points come from the workbooks; a refusal offers
  *Use the suggested points* and *Use this exam's points* (`points_by_form`, checked against
  every Remark total); points and maxima that differ from the exam's are listed first under
  TOTALS. The comparison alone holds nothing, and its subtitle says so: *Queue for review*
  (`ExamProject::queue_parallel_differences`) puts each settled difference from an
  auto-accepted answer in Review (MRK-001 `parallel_run_disagreement`), where it holds release
  until decided; the subtitle then names the differences waiting in Review. A `.csv` among the
  workbooks typed into the Compare field is read as handwritten student numbers
  (`file,page,student_id`, as `omr compare remark --handwritten`).
- Head-to-head figures of real exams never enter the repository (§3.5, Q-08): the tests and
  screenshots use synthetic workbooks only (`tests/support/synthetic_remark.rs`).

**Alignment refusals** (REG-003 ERROR, F1): a page whose alignment with the blank form cannot be
verified (a bubble lattice can match one row or column away, so nothing on it was read) is
counted in the reconciliation ("Not read: alignment unverified (REG-003)"), listed in the activity
log with its remedy, and its review notice gains a *What to do* line: rescan flat (a phone photo:
whole sheet in view, from straight above, evenly lit) and add the new scan, never key its
answers from that page: the core's text (`omr_app::review::remedy("REG-003")`, R10).

**Flag stability** (§12.2, §12.3). Results' item analysis has a *Stability* column: the share of
1,000 bootstrap resamples in which the flag's rule fires again (`omr_analytics::stability_job`,
seeded from the analysis content). High confidence shows the share in a success tag; low
confidence (not above 50 % beyond Monte Carlo noise, or failing at one end of its 95 % CI) reads "31 % · proposed" in gold, the flag pill
turns gold and ends "(proposed)", and the row's label carries the job's account ("fires in 31 %
of 1,000 bootstrap resamples; fails at the upper end of its 95 % CI"). Rows sort mis-key and
flawed-item flags (ranks 1–4) of high confidence first, then those of low confidence, then the
informational ones, each by rank then share (the spec's "sort by rank, then stability", with
confidence ahead of rank so a proposed mis-key never sits above an established flag). The job
runs off the synchronous regrade path, on a thread the binary's spawner starts, once per score
run ("computing…" until it reports; `pump` folds it in); without a spawner (headless tests) it
runs in place. When omr-app's `analytics()` carries its own stability report, that one is used.
When the job reports, the shell hands it to the core (`ExamProject::set_flag_stability`), which
judges the probable mis-keys for the release gate: a confident one becomes a Scoring-lane
SCO-001 review item (ERROR) that blocks *Release results…* until it is decided, and until the
job has reported the release checklist reads "Mis-key check … computing…". In Review, the item's
letter keys change the key to that option, `M` accepts the key or the flagged option, and `A`
keeps the key, asking for a reason code and a note (voiding is done with `omr review resolve
--decision void` or by setting the key entry to `*` in Setup).
A real session has no what-if preview: the SCO-001 help points to a key change in Setup, which
can be undone there (the demo keeps its *What-if preview* button and Ctrl+P, which say so).

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
| Results | `results_vm` plus `analytics()` (α, SEM, item flags with codes, rank, keyed option) and the stability job's report; `hold()` for an exam waiting for its key |
| Setup · key sheets | `key_sheet_summary(true)` of the exam, or of key-sheet files read with `keysheet::read_key_sheets`; `confirm_key_sheets`, `replace_key` |
| Outcomes | `outcomes()` (μ, π and their CIs, verdict, policy and version) and `outcomes_vm` (TP-154 B.1 cells) |
| New exam · Setup · Remark import | `remark::read_workbook_files`, `import`, `import_files` (a points file), `suggest_points`; `RemarkImport` (versions, points, groups, key-sheet rows, workbooks, notes); applied with `set_key_points`/`outcome_setup` (new exam) or `replace_key_points` (Setup) |
| Results · parallel run | `ExamProject::compare_remark` (with crops), `ParallelRun` (counts, disagreements, identity, students, key-sheet effects, workbooks), `export_parallel_run` |

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
are blank sheets. A control with nothing behind it is not shown in a real session: the What-if
preview, Queue filter and Rescan pull list buttons (and Ctrl+P, Ctrl+R) are the demo's only, where
they say that they are not built yet and name the command that does it (`omr review list --lane
capture`, …), and the Settings keymap lists them there only. No message of a real session ends in
a command line. The Exams search filters the table (course code, titles or assessment); Roster
search takes a student number on an identity item and says so otherwise, and shows only with an
exam open. A click on a sheet read automatically says there is nothing to review; a sheet with an
open item opens that item, never the first item of its code (another student's). The code help
(F1) of a code with no help of its own is its catalogue title and the core's remedy. New exam on
demo data says how to open real exams; the TP-154 button writes the exam's exports
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
- item-analysis rows may arrive in any order: the shell lists flagged items first (ranks 1–4 of
  high confidence, then of low confidence "proposed", then the informational flags; each by
  rank, then share, then question);
- key-sheet rows never auto-accept: a row that is not one confidently read option has no key
  entry until the instructor decides it, and the confirmation names any row left undecided.

## Synthetic exam and evidence

`tests/support/synthetic_exam.rs` paints pencil marks (omr-sim's mark painter) on the committed
blank IAU form, adds a light scanner model and grades through omr-app: CS 211 Midterm 1,
60 sheets, versions A and B, 30 questions, a 62-student roster with invented English and Arabic
names (IDs start with 9), four CLOs with split weights and an Arabic note. The mix gives 14 open
items: 10 light marks (MRK-001), a double mark (MRK-002, `multi_review = always`), a blank
version bubble (SCO-005, form inferred), a number not on the roster and a faint ID digit
(IDN-004), plus a mis-key on form A Q12 that item analysis flags. A second exam (CS 212 Quiz 2,
24 sheets) is graded with an inferred key. A third (CS 214 Final, 24 sheets) is graded with the
instructor's synthetic key sheets named and no key file (held): version A (student number
`0000000000`) carries a double mark, an empty row and a faint mark, version B (`9999999999`) is
clean. No real scan, name, student number or key is used.

`tests/support/synthetic_remark.rs` writes synthetic Remark graded workbooks of the midterm's
own sheets in the documented export layout (the example puts them in
`<workspace>/remark-cs211-midterm-1/`): versions A and B with learning-objective groups whose
exact points differ per version (0.5, 0.75 and 1; 25 points each), the instructor's key sheet
graded as a student (`0000000000`), a clear mark Remark read as another option (sheet 6), a
light fill Remark left blank (sheet 9), a booklet with no row (sheet 50), a row whose ID Remark
could not read (`ERROR …`, a student with several wrong answers, so the run pairs it by
answers), an Excel-typed ID, the roster number where a sheet bubbles
another, an absent student's row, and another section's workbook; plus a small two-version
export whose totals leave two questions' points open (the refusal and its suggestion). Never a
real export.

| Test | What it proves |
|---|---|
| `req_a11_real_backend_review_flow_rescores_results` | the synthetic midterm is graded, opened with `AppBackend` and reviewed through accessible labels: accept a suggestion, choose the keyed answer (an override: the reason chip first), mark a blank, undo it, resolve an identity as not on roster, accept an inferred form; each step's booklet percentage and the class mean move exactly as the core rescored them, the reason is in the audit record, and the results view-model equals a full rebuild from the saved project; every remaining item settled, then *Release results…* and *Release* release the exam (the Blackboard upload is written); the Exports bar writes the Blackboard upload into column 835932 (usernames and scores only, no names) and every export |
| `req_a11_real_backend_grades_live_through_pump` | a background grading run (the test owns the worker) drives the Scan screen through `pump`; empty states before the project exists; afterwards the batch is complete with real thumbnails and results |
| `req_a11_real_backend_adds_scans_from_the_scan_screen` | day one graded, one decision taken, then *Add scans…* with day two (a portrait scan and a blank back) through accessible labels: while the worker runs the exam is busy (commands refused, queue says why, row *Scanning*); afterwards batch 2 of 2 with 8 booklets, the REG-002 and CAP-012 notices, the decision kept |
| `req_a11_real_backend_edge_projects_never_panic` | a project without roster or CLOs, graded with an inferred key: every screen and key, Blackboard disabled with the reason, no invented roster; an override with Other and its note; a typed student number; then every item settled and the empty queue (undo re-opens) |
| `req_a11_real_backend_empty_workspace_never_panics` | an empty workspace: every screen and key, empty states |
| `req_a11_real_backend_replaces_the_key_from_setup_and_names_a_tampered_project` | an inferred key replaced from Setup (*Replace key…*, the key file, *Replace and rescore*): the key is in force and no longer INFERRED; a snapshot edited outside the app is refused with SYS-011 and the remedy, never "close the other program" |
| `req_a11_real_backend_creates_an_exam_from_the_new_exam_form` | *New exam* on a workspace: a question count that is not a number and a missing key file are questions on their fields (said in the field's description) and Grade exam grades nothing; then, with no key, the provisional key chosen on the card, the scans graded into a new folder of the workspace in the background; one run at a time (the next card waits until it ends) |
| `req_a11_real_backend_new_exam_reads_the_named_key_sheets` | *New exam* with the instructor's key sheets in its *Key sheets* field: they are read with the exam as stored key sheets (six students graded, two key sheets), the run held until *Confirm every version* in Setup, then scored; the key version names both key-sheet scans (C7) |
| `req_a11_real_backend_confirms_the_key_from_key_sheets` | the synthetic final graded with its key sheets: held (no results; Results' empty state leads to Setup); Scan lists both key sheets as key sheets, not students (thumbnail labels, reconciliation, activity); Setup opens on the summary with the three rows of version A highlighted (several marks, no mark, not read clearly) and every row's crop; *Confirm version A* stays disabled until each is decided through its labels (the intended answer, *Void*, the answer); A confirmed alone keeps the hold; *Confirm version B* publishes the key and scores the exam; both confirmations recorded with the operator and the scans; the scores equal grading the same students with the same key as a file |
| `req_a11_real_backend_reads_a_key_from_answer_sheets_in_setup` | an inferred-key quiz: *Read key from answer sheet…* with a student's sheet gives no key (no marker) and *Put aside* leaves the key unchanged; with the two key sheets the summary is shown and nothing changes until *Confirm every version*, which puts the instructor's key in force (no longer INFERRED, every entry equal to the sheets) |
| `req_a11_remark_import_on_the_new_exam_form_grades_with_its_key_points_and_objectives` (`req_a11_remark.rs`) | *Import from Remark export…* on the New-exam card with three synthetic workbooks (versions A and B, another section's A): the key hidden (rows carry no letter) until *Reveal key*, every question's exact points (version B's other order: Q01 0.5 points, LO2), four learning objectives "imported, needs review", the key sheet excluded (and a student with every answer right reported for a check), IDs masked; *Use this import*: the card's key, 30 questions and versions A, B come from the import; a key file beside it is a question (*Use the Remark import* clears it); graded: keys, points per version and the CLO rows (`Remark import`, per version) equal the workbooks'; the small class leaves learning-objective points open, so the refusal suggests the equal split and *Use the suggested points* gives it; in Setup *Approve imported mappings* records one audited approval and the TP-154 draft watermark goes |
| `req_a11_remark_import_refusals_name_the_remedy_and_the_suggested_points` | a `.rmx` file refused unopened with the remedy (and nothing to use); a version whose totals leave questions 2 and 3 open refused naming them (no `--points` in the desktop's words) with 0.75 each suggested; *Use the suggested points* imports with "given by you", 6 of 6 totals reproduced |
| `req_a11_remark_import_in_setup_replaces_the_key` | a quiz graded with an inferred key: Setup's import of an export with other points is shown as not applicable, with why (a graded exam's points are fixed with its stored form map) and what to do, *Apply* disabled; the export with the exam's points is shown (nothing changes), back to the matrix and to the waiting import, *Apply key and points*: key version +1, no longer INFERRED, every entry equal to Remark's Answer Key rows, the message says the objectives were not applied |
| `req_a11_parallel_run_in_results_shows_the_differences_with_crops` | the synthetic midterm beside its three workbooks: the Restricted / Q-08 banner, tiles equal to omr-app's own comparison, the settled difference Remark read otherwise first with its bubble-row crop, the booklet with no row and the row with no student number (masked), the key sheet, the other section not compared; *Save report* writes every file with the banner and the manifest; the subtitle says the comparison holds nothing, *Queue for review* queues each settled difference (open blocking items rise by as many), the button goes and the subtitle names what waits in Review; *Back to results* |
| `req_a11_remark_states_fit_the_minimum_window_in_both_themes` | the three states at 1280 × 800, light and dark: every control labelled once and inside the window |
| `no_host_hides_browse_buttons` (`req_gui_workflow.rs`, real clicks and typed keys, a fake host) | without a host no Browse button exists and typed paths grade; with one, every path field of the New-exam card has its buttons (the sheet's and the output folder's under *Advanced options*), and each says which dialog it opens: the output folder's a folder dialog, the others a file dialog |
| `browse_fills_the_scans_field_with_the_picked_files` | three scans picked (one named `batch;1.jpg`) are checked and graded as picked; the dialog offers the scan types, opens in the exams folder, then where the scans were picked (saved in the preferences) |
| `a_cancelled_dialog_keeps_the_typed_text` | nothing picked leaves the typed text, says once to type the path if no dialog appeared, and the next click opens a dialog |
| `editing_a_picked_field_falls_back_to_typed_paths` | picked files typed over with the folder: the folder is graded |
| `browse_clicks_wait_for_the_open_dialog` | one dialog at a time; its answer fills its own field; a dialog that could not show does not block the next |
| `roster_and_clo_browse_use_their_filters` | each New-exam file field's file types (key, roster, CLO, key sheets, the sheet's spec under *Advanced options*) and the Remark panel's (graded exports only) |
| `setup_replace_key_accepts_a_quoted_path` | Setup's key file quoted as "Copy as path" gives it, then a corrected key picked in the dialog: both in force |
| `setup_scan_and_parallel_run_fields_browse` | Setup's CLO, key-sheet and Remark fields, Scan's added scans (files or a folder), the parallel run's workbooks (picked from a folder whose name holds a `;`) and report folder |
| `prefs_reads_the_old_workspace_only_file`, `prefs_ignores_unknown_keys_and_bad_json`, `prefs_round_trip` (`host.rs` unit tests) | the preferences file of earlier versions (the workspace only) reads as version 1 with the defaults; unreadable, unknown or mistyped entries take their defaults; every preference survives a save and a read |
| `browse_buttons_fit_the_smallest_window` | at 1280 × 800 in both themes, every Browse… and Folder… button (New exam, its Remark import, Setup's bars, Add scans, the comparison and the parallel run) is inside the window and leaves its field room for a path |
| `the_survey_runs_at_once_and_fills_the_card` | a picked folder is checked at once (progress, *Stop checking*, Grade exam waiting with why); then every value shows its source and evidence, the blank fields show the value they will grade, the "no answer key" question has its four answers and blocks Grade ("Answer key: choose the key file, …", the label said once), rows read with their marker (", question", ", note"), the notes stay folded until *Show notes (N)* |
| `an_ask_blocks_grade_and_a_choice_unblocks_it` | a click on Grade exam while a question is open grades nothing; *Grade with a provisional key* answers it (the key "provisional — your answer"); other scans drop that answer and ask again; the exam graded has an inferred key only after the second choice |
| `the_graded_definition_equals_the_proposals_inputs` | typed title, course, versions and (under *Advanced options*) term: the definition graded equals `define` of the inputs `propose` gives on its own for the same scans and answers, in the card's folder, with each value's source recorded |
| `editing_the_scans_discards_the_old_survey` | a typed title reaches the card only once typing pauses, without a new survey; other scans start one, and a survey of scans no longer on the card that ends later is dropped |
| `advanced_options_are_collapsed_until_clicked` | the sheet, output folder, term, student-ID scheme, cancel mode, second review and high-stakes fields exist only once *Advanced options* is opened (and are gone when folded); the IAU sheet's cancel mode is locked, showing "Erase to change · the sheet"; second review is chosen from "No second review", "Always" and "On high-stakes exams" (a click grades it, nothing asked); the high-stakes switch reads its state first |
| `a_spec_file_that_prints_its_cancel_mode_fixes_the_field` | an own sheet's spec file that prints `cross_cancels` locks the cancel mode as the IAU sheet does, showing "Cross out to cancel · spec file" |
| `an_answer_holds_only_while_its_field_keeps_its_value` | *Use 5* for 9 options, then 4 typed: 4 is graded; *Use 30* for 25 questions, then 20 typed: 20 is graded and the rows beyond it are asked again |
| `a_typed_folder_name_grades_into_the_exams_folder` | "quiz-rel" typed as the output folder is shown, graded and listed in the exams folder, not the program's working folder |
| `emptying_the_scans_drops_the_answers_given_for_them` | an answer given before any scans stays; the provisional key chosen for some scans is asked again once they are emptied and others typed |
| `the_card_opens_during_a_run_and_says_why_grade_waits` | while one exam is graded the card opens, checks its scans and says why Grade exam waits; when the run ends Grade is available, into another folder than the first exam's |
| `stopping_the_survey_holds_grade_until_checked_again` | *Stop checking* ends the check: a question on the scans blocks Grade, the stopped worker's end changes nothing, *Check the scans again* checks them |
| `an_invalid_value_is_marked_on_its_field` | "thirty" questions: the question shows under the field (its description) and blocks Grade; *Type the count* puts the keyboard in the field, and 30 typed there unblocks it |
| `screenshots_make_the_card_from_an_exams_files` | the card shots type the exam's files as an instructor types them (`~` for the home folder) and wait for the check of the scans: the scans alone ask for the answer key and Grade exam waits; with the key, roster and CLO files nothing is asked, title and course come from the scans' folder name and the card's folder reads `~`; the Browse buttons are shown; the card as it opens is empty |
| `the_notice_lies_over_no_control` | at 1280 × 800 the card opened during a run keeps Grade exam clear of the run's notice, grading from it takes that notice down, and the exams drawer's buttons are clear of the next one |
| `the_notice_is_dismissed_with_escape_and_the_keys_keep_working` | Escape dismisses the notice; dismissed with a click on ×, Ctrl+5 still opens Results |
| `review_opens_on_a_lane_with_items` | a lane with nothing open names the lanes that have items, and Review shown again opens one of them, never "Nothing to review" |
| `exports_older_than_a_later_decision_are_said_to_be` | after the roster run wrote the exports, one decision makes the notice and Results' Export… say they are older; Write all exports makes them current |
| `opening_new_exam_takes_down_the_last_runs_notice` | at 1280 × 800 New exam takes the last run's notice down, so Grade exam takes the click |
| `answering_on_the_card_keeps_the_keyboard` | once a question's answer (a dialog's pick or a one-click answer) removes it with the button that had the keyboard, Grade exam takes the keyboard: Escape closes the card, Ctrl+4 works, Return grades |
| `the_close_dialog_keeps_the_keyboard`, `escape_keeps_the_run_and_gives_the_keys_back` | Tab and Shift+Tab stay on the close dialog's two buttons; Ctrl+4 does not reach the screen under it; Escape keeps the run going and Ctrl+1 works again |
| `an_unsaved_name_is_not_shown_as_recorded` | a name typed in Settings but not saved says so, decisions keep the saved name, and Settings shown again shows the saved name |
| `setup_offers_add_roster_while_the_exam_has_none` | at 1280 × 800 Setup's top bar has Add roster… beside Replace key…; it opens the roster bar and goes once the roster is added |
| `screenshots_show_the_notice_on_its_shot_only` | a grading run ended unseen: scan-notice shows its notice (in both themes), every other shot hides it; the Scan log names the project saved by its last folders, as the status bar does |
| `screenshots_of_an_empty_folder_show_its_empty_state` | an empty exams folder has one shot, exams-empty, with New exam…, Open another folder… and the Remark import |
| `the_proposed_report_folder_reads_a_tilde` | the parallel run's proposed report folder below the home folder reads `~/…` and is read back as the same folder (`bind::home_path`) |
| `settings_keymap_lists_the_folder_keys` | the Settings keymap lists Ctrl+O and F5 where an exams folder can be opened |
| `req_a11_demo_data_is_marked_on_every_screen` (demo file) | the DEMO DATA mark on every screen, release refused on demo data, and New exam saying how to open real exams |
| `req_a11_blind_items_never_show_a_leaked_engine_reading` (demo file) | a backend that sends the engine's reading with blind items: the frame, the sparkline and Enter never show or commit it |
| `req_a11.rs` (demo) | the review flow, keyboard paths, labels, layouts and screens on the demo backend |

The review-flow test also checks the item analysis: form A Q12 (the planted mis-key) is flagged
with high confidence by the stability job and its row comes first; every "proposed" flag comes
after the high-confidence ones and its pill ends "(proposed)".

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
Ctrl+K, Ctrl+N and Ctrl+O on Exams, and F5 there (whose table is one tab stop: ↑/↓ move, Enter
opens the drawer, Esc closes it); Ctrl+Space and Ctrl+R on Scan; Ctrl+P on Results. A page change hands focus back
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
