# Mizan (ميزان)

Exam correction and outcome evidence. Mizan was called OMR Correction while it was being
designed; the code keeps that working name in its identifiers (the `omr` command, the `omr-*`
crates, `.omr`/`.omrproj` projects and the `OMR_*` settings), so existing projects and scripts
keep working.

A native, local-first program that grades multiple-choice answer sheets from scans, built for
Imam Abdulrahman Bin Faisal University (IAU) and developed by AI agents. It reads the IAU CCSIT
answer sheet and sheets the university generates itself, matches each sheet to a student,
scores it, sends every doubtful mark to a person instead of guessing, and writes the results,
item analysis, NCAAA TP-154 outcome tables and the Blackboard grade-center upload. Every
decision is kept in a signed, hash-chained audit log in the exam's folder.

**Status: MVP (October 2026).** Grading, review, release and exports work from the `omr`
command line and from the desktop app `omr-desktop`, on Linux.
- What was measured, and how: [`docs/EVIDENCE.md`](docs/EVIDENCE.md). On 143 real IAU sheets
  the software made no silent error in 4,089 answers it accepted, and sent 7 to review.
- With the instructors' real keys, every fully scored booklet of the two real exams (137 of
  143) scored exactly as an independent count of its marks against the key; the other 6 wait
  only for their 7 questions in review. The keys read from the instructors' own key sheets
  gave the same results, file for file.
- A parallel run with Remark Office OMR (spec §23.7) was performed internally on the two real
  exams with `omr compare remark`, grading them with the points imported from Remark's own
  graded workbooks. It was a development check by the agents on data the owner supplied: the
  licence review the spec requires before parallel runs and benchmarking (Q-08, the
  university's legal affairs, due 2026-12-01) is still open, so its head-to-head figures stay
  inside the university (spec §3.5).
- **Windows:** CI runs the same gate (formatting, clippy, every workspace test, the
  architecture rules) on Windows Server 2025 (MSVC) as on Linux; the Unix file-mode checks are
  skipped there. Still to build for Windows:
  - refusal of OneDrive and other sync folders;
  - owner-only file permissions;
  - DPAPI-protected keys;
  - an installer.

  The desktop app has not yet run in a real window (see `docs/IMPLEMENTATION.md`).
- **Not proven yet:** identity against a real roster.
- What the MVP covers and what it defers: [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md).
  The full design: [`docs/TECHNICAL_DESIGN.md`](docs/TECHNICAL_DESIGN.md).

Scans of real exams are restricted personal data (PDPL). Keep the exam folders on university
devices, never in a synced cloud folder: the program refuses sync folders such as OneDrive,
Dropbox, Nextcloud and iCloud, and network shares (SYS-015). On Windows, so far only network
(UNC) paths and the Dropbox-style marker files are refused (see Status).

## 1. Build

You need:
- **Rust** 1.92 or newer (tested with 1.97), installed with [rustup](https://rustup.rs).
- **Linux:** a C compiler, `pkg-config` and the fontconfig headers to build the desktop app,
  and libxkbcommon-x11 to open its window. On Debian or Ubuntu:
  `sudo apt install build-essential pkg-config libfontconfig1-dev libxkbcommon-x11-0`.
- **Windows:** the same `cargo build` works with Visual Studio Build Tools (the "Desktop
  development with C++" workload, for the MSVC linker and the bundled SQLite). CI builds and
  tests it on Windows Server 2025. Read §8 before grading real exams on Windows.
- **macOS:** not built or tested yet.

```sh
git clone <this repository> omr-correction
cd omr-correction
cargo build --release -p omr-cli -p shell-slint
```

This makes two programs: `target/release/omr` (the command line) and
`target/release/omr-desktop` (the desktop app). Copy them to a folder on your `PATH`, or call
them by their full path. `omr --help` and `omr <command> --help` list every option.

**Test builds.** The repository's Releases page has unsigned pre-releases (`v0.1.0-preview.1`
and later) for x86-64 Windows (a zip) and Linux (a tar.gz), each with both programs. They are
test builds, not production releases, which will be built and signed in the university's
release enclave (spec §24.2). Windows SmartScreen warns about unsigned programs; read §8 before
grading real exams on Windows. To publish the next one, run the *Release builds* workflow on
main with a new tag; `scripts/release-tag.sh` checks it.

## 2. Grade a folder of scans

Scan each exam into one folder: PDF or TIFF straight from the scanner (600-dpi black and white,
or 200–300-dpi gray), or JPEG/PNG files. Blank backs from duplex scanning are dropped, turned
pages are read upright, and a file added twice is ignored.

### With the answer key

```sh
omr grade scans/cs211-midterm/ --template iau-ccsit-v1 --questions 30 \
    --key key.csv --roster roster.csv --clo clo.csv \
    --course "CS 211" --title "Midterm 1" \
    --out "$HOME/OMR Exams/cs211-midterm-1"
```

- `--template iau-ccsit-v1` is the IAU CCSIT answer sheet; `--questions` is how many of its
  questions the exam uses.
- `--key` is the answer key, one column per exam version (A, B): see
  [`docs/examples/key.csv`](docs/examples/key.csv). The versions come from the key file. A
  single `key` (or `answer`) column is the key of every version, so a single-version exam
  needs nothing more: a sheet whose version bubble is blank is graded with that key, without
  review. A sheet that bubbles a version the exam does not have (C on an A, B exam; B on an
  exam with `--forms A`) is held for review and never scored until you choose its version.
- `--roster` (CSV or Excel `.xlsx`, [`example`](docs/examples/roster.csv)) and `--clo`
  ([`example`](docs/examples/clo.csv)) are optional. Without a roster, sheets are identified
  by the student number bubbled on them; without a CLO file there are no outcome tables.
- `--out` is a new folder for this exam. It is created readable by you only and holds the
  exports and the exam's audited project (`project.omrproj`: do not edit or move files inside
  it; an edited project is refused with SYS-011).

The last lines say what to do next, for example
`release blocked: 4 ERROR/BLOCKER open; next: omr review list --project …`, or
`ready to release: omr release --project …`.

### Without an answer key

```sh
omr grade scans/cs211-midterm/ --template iau-ccsit-v1 --questions 30 --infer-key \
    --out "$HOME/OMR Exams/cs211-midterm-1"
```

Each version's key is then inferred from the most common confident answers. Every output is
marked INFERRED and can never be released. When the real key arrives:

```sh
omr key replace --project "$HOME/OMR Exams/cs211-midterm-1" --key key.csv
```

### With a key sheet

Fill one answer sheet per version with the right answers (the key sheet), bubble its student
number all 0 (or all 9: numbers no IAU student has) and its version, and scan it with the
students, anywhere in the stack, or give it on its own:

```sh
omr grade scans/cs211-midterm/ --template iau-ccsit-v1 --questions 30 \
    --key-sheet key-A.pdf --key-sheet key-B.pdf --out "$HOME/OMR Exams/cs211-midterm-1"
```

A key sheet is never graded as a student, and its answers are never accepted on their own:
nothing is scored until you open `key_sheet_summary.html` in the exam folder (each question's
answer as read, beside the scanned bubble row; rows to check are highlighted) and confirm it:

```sh
omr key confirm --project "$HOME/OMR Exams/cs211-midterm-1"
omr key confirm --project … --answer 7=B --answer '12=A|C'   # decide highlighted rows first
```

Each row of the summary shows the bubbles as scanned with the question number printed beside
them: that number must match the question column (if not, the sheet was read a row off: do not
confirm, rescan it flat). A key sheet that cannot be read reliably (for example a photo
registered one bubble row off, whose student number then reads 1111111111) is refused and
named, never used; so is a key sheet whose version is not bubbled, on an exam with several
versions. If a version is given by more than one sheet found in the stack, the summary and the
hold name every one of them, even when they agree: a student who bubbled all 0 by mistake and
answered like the key reads the same as a key sheet. Check each one; if one is a student's,
grade again naming the real key sheet (`--key-sheet key-A.pdf --key-sheet-marker none`) so
that the student is graded. A correction made after confirming (`omr key set`) stays your
decision when the key sheet is scanned again later (shown as "decided by you" in the next
summary).

Key sheets you get after grading join the exam the same audited way:
`omr key replace --project … --from-key-sheets key-A.pdf key-B.pdf`, then `omr key confirm`.
`omr key read key-A.pdf key-B.pdf --template iau-ccsit-v1 --questions 30 --out key/` only
previews them: it writes `key/key.csv` and `key/key_summary.html` and records nothing.

### From Remark Office OMR

Programmes that graded with Remark Office OMR bring its graded Excel exports (the Advanced Grade
reports exported to `.xlsx`; Remark's own `.rmx` and `.aky` files are never read):

```sh
omr import remark Sec1_A_Graded.xlsx Sec1_B_Graded.xlsx Sec2_A_Graded.xlsx --out remark-import/
omr grade scans/ --template iau-ccsit-v1 --questions 28 \
    --key remark-import/key.csv --clo remark-import/clo.csv --out "$HOME/OMR Exams/final"
```

The import shows a summary first, then writes the key of each version with its exact points
per question (from Remark's learning objectives, or solved from the students' totals, and
checked to reproduce every total Remark reported), the learning objectives as CLO mappings
marked "imported, needs review", a version map and a signed `manifest.json`. When the totals
cannot tell some points apart (two questions every student answered right, say, or a learning
objective whose questions the same students got right: its equal split would be a guess) it
stops, names them and suggests values: give them with `--points points.csv`
(`question,points` or `question,points_A,points_B`). Rows Remark graded from the instructor's
key sheet (student number `0000000000`) are reported, never students. An AND key (`A+C`,
Remark's partial-credit syntax) is refused: it is not scored yet.

The summary gives each learning objective's points and questions per version and warns when
they differ between the versions (Remark groups printed positions, so one objective may not
mean the same items in every version); every CLO is written in domain K (the export has no
domain). Correct `clo.csv`, then, once the exam is graded, approve the imported mapping (the
TP-154 tables are a draft until then) or replace it, without grading again:

```sh
omr clo replace --project "$HOME/OMR Exams/final" --clo remark-import/clo.csv
omr clo approve --project "$HOME/OMR Exams/final" --approver "Dr. A. Coordinator" \
    --note "objectives checked against the exam paper"
```

To check a new exam against Remark before leaving it (spec §23.7), grade the same sheets in
both and compare:

```sh
omr compare remark --project "$HOME/OMR Exams/final" Sec1_A_Graded.xlsx Sec1_B_Graded.xlsx \
    --out "$HOME/OMR Exams/final/parallel-run"
```

`parallel_run.html` lists every answer where the two differ, with the scanned bubble row to
decide it, this exam's own total per student (its points and voids: what it will release)
beside Remark's, any question whose points differ from Remark's (a different maximum is
warned about: no total can agree then), the student numbers that do not match, the learning
objectives' subtotals, and the key sheets Remark counted as students. The report alone does
not hold release; add `--queue` to put every settled difference from an answer this exam
accepted on its own into Review, where each holds release until you decide it (your decisions
are recorded as field labels). The report is Restricted: its figures stay inside the
university. It states the licence review (Q-08) as recorded by the administrator in
`licence_review.json` in the private key folder (`{"q08": "cleared", "by": "…", "date": "…"}`,
or `"refused"`, which stops every comparison); none recorded means the review is open. Give it
the workbooks of the sheets you graded (other sections' workbooks are listed and left out);
workbooks in which no student's answers match a booklet are refused as another test's, and a
student number whose answers are another sheet's is paired last and flagged.

### Later batches and the university's own sheets

- Another section or another day: `omr add more-scans/ --project <exam folder>` grades the
  exam again with every scan, keeping your review decisions.
- Own sheets: `omr templates --example-spec > spec.json`, edit it, then
  `omr sheet render --spec spec.json --out sheets.pdf --copies 120`. The exam's secret key file
  is written to your private key folder (§7), not beside the PDF; grade with
  `--spec <that key file>` instead of `--template`.

## 3. Review

Anything the program is not sure about waits for a person: faint or double marks, erasures,
unreadable student numbers, a student not on the roster, two sheets with one number.

```sh
omr review list --project "$HOME/OMR Exams/cs211-midterm-1"
omr review resolve --project … --item 12 --decision B
omr review resolve --project … --item 15 --decision C --reason AMBIGUOUS_MARK
omr review resolve --project … --undo
```

- `review list` shows the open items in queue order, with student numbers masked to their
  last 3 digits (`--reveal` shows them in full, and the reveal is logged). `--crops DIR` saves
  each item's image as PNG; `--json` is for scripts.
- Decisions: `ack` accepts the program's proposal (or acknowledges a notice); otherwise a
  letter (`B`), `blank`, `multi:A+C`, `unreadable`, `student:<number>`, `not-on-roster` or
  `form:B`. Choosing something other than the proposal needs a reason (`--reason`, e.g.
  `AMBIGUOUS_MARK`, `ERASURE_RESIDUE`, `ID_CORRECTION`; `OTHER` with `--note`).
- A wrong key entry: `omr key set --project … --form A --question 7 --answer C`. Every change
  rescores the exam and is recorded with its reason.
- A **probable mis-key** (SCO-001, Scoring lane): the item analysis found a question whose key
  the strongest students contradict, with confidence (its bootstrap stability). It blocks
  release until you decide it: the letter of the right answer (changes the key), `multi`
  (accepts the key or the flagged answer), `void`, or `ack --reason ADJUDICATION --note "…"`
  (keeps the key). A low-confidence flag (one student's choice, for example) is shown as
  *(proposed)* in the item analysis and never blocks.
- Phone photos of students' sheets are read, but the phone capture is not calibrated yet:
  every answer on them waits for your confirmation (MRK-001 `uncalibrated_capture`). Scan
  students' sheets on a scanner.
- A key read from key sheets waits for `omr key confirm` (§2): until then `review list` shows
  the hold first, and nothing is scored or releasable.

## 4. Release and upload

```sh
omr release --project "$HOME/OMR Exams/cs211-midterm-1"
```

Release is your deliberate act. It is refused, naming each reason and the next command, while
anything blocks it (open review items, a probable mis-key not yet decided, an inferred or
incomplete key). It then writes the exports
again, now with `blackboard_grade_center.csv` (username and score only) for Blackboard's Grade
Center upload. A later change (a review decision, a key change, an added batch) supersedes the
release: the upload file is removed at the next export, until you release again.

The exam folder holds `results.xlsx` and `results.csv` (scores, percentages and Saudi letter
grades), `item_analysis.xlsx`, `tp154.xlsx` (NCAAA course-report tables), `report.html`, and
`manifest.json`, which lists every file's SHA-384 and is signed with this computer's key.
`omr export --project …` writes them again.

## 5. The desktop app

```sh
omr-desktop                                   # your exams folder: ~/OMR Exams, or the last one
omr-desktop --workspace "$HOME/OMR Exams"     # every exam below a folder
omr-desktop --project "$HOME/OMR Exams/cs211-midterm-1"
omr-desktop --demo                            # invented demo data, marked DEMO on every screen
```

- **Exams** lists every graded exam. **New exam** (Ctrl+N) grades a folder of scans into a new
  exam folder: the scans, the sheet (`iau-ccsit-v1`), the number of questions and, optionally,
  the key, roster and CLO files. Without a key file the key is inferred.
- **Setup** shows the key, versions and CLO mapping; **Replace key…** loads the real key from
  a file and rescores; **Read key from answer sheet…** reads the instructor's key sheets, shows
  the key summary and, once you confirm, adds them to the exam (stored and audited like key
  sheets graded with it). New exam also takes key sheets.
- **Scan** follows grading live; **Add scans…** adds a later batch.
- **Review** is keyboard-first: the number or letter keys choose an answer, Enter accepts the
  proposal, Ctrl+Z undoes; F1 lists every key.
- **Results** shows scores and grades; **Release results…** releases them and writes the
  Blackboard upload. **Outcomes** shows CLO attainment and the TP-154 tables.
- **From Remark Office OMR:** **Import from Remark export…** on the New exam form (or in Setup,
  to replace a graded exam's key) reads Remark's graded `.xlsx` exports and shows the import for
  review before anything changes; a refusal says what to do and can offer the suggested points.
  In Setup, **Approve imported mappings** approves the imported CLO rows and **Replace CLOs**
  loads a corrected CLO file. In Results, **Compare with Remark export…** shows the parallel
  run under the Restricted banner (a `.csv` among the files is read as handwritten student
  numbers, `file,page,student_id`); **Queue for review** puts its settled differences into
  Review, where they hold release; **Save report** writes the Restricted report.

On Linux the app needs a desktop session (X11 or Wayland). On a server, use the `omr` command
line, or render the screens to PNG files with `omr-desktop --screenshot DIR`.

## 6. File formats

| File | Layout |
|---|---|
| Answer key | CSV `question,A,B` (one column per version, optional `points` column, or `points_A`, `points_B` when the versions carry different points), or `form,Q1,Q2,…`, or one column of answers. Entries: `B`, `A\|C` (either accepted; also `A or C`, `AC`), `*` (void), empty (missing); an AND key (`A+C`, `A&C`, `A and C`: all answers required, Remark's partial credit) is refused, not scored yet. UTF-8, UTF-16 or Windows-1256; commas, semicolons or tabs, as Excel saves it. Or a key sheet: an answer sheet filled by the instructor, student number all 0 or all 9 and the version bubbled (§2) |
| Roster | CSV or `.xlsx` with a header row: `student_id` (or `ID`, `الرقم الجامعي`), `name`, `name_ar`, `section`. Student numbers that Excel damaged (`2250000001.0`, lost leading zeros) are repaired when nothing was lost; duplicates are refused naming the rows |
| CLO mapping | CSV `clo,domain,title,plo,questions,weight,note`, with question lists such as `1-8,12`; optional `form` (the row maps one version's questions) and `status`/`source` (`imported, needs review`, `Remark import`) |
| Remark export | Remark Office OMR's graded workbook (`.xlsx`): Student Response Report with the answer key, Student Statistics Report (learning objectives), Test Statistics Report; `omr import remark`, `omr compare remark` |

Examples: [`docs/examples/`](docs/examples/). More detail: [`crates/omr-cli/README.md`](crates/omr-cli/README.md).

## 7. Where things are kept

- **The exam folder** (`--out`, or a folder below your exams folder): the exports and
  `project.omrproj`, the audited store of every page, decision and key version. Back up the
  whole folder.
- **Your private key folder:** `~/.local/share/omr` on Linux (`%APPDATA%\omr` on Windows, or
  `OMR_KEY_DIR`). It holds this computer's signing key, the checkpoints that detect an older
  copy of a project put back in place, and the secret keys of the university's own sheets. Back
  it up separately and keep it private.

## 8. Known limits

- macOS is not built or tested. Windows is built and tested in CI only, without its platform
  layer (sync-folder refusal, owner-only permissions, DPAPI keys), and there is no installer yet.
- The scanner's hot folder is not watched (grade the folder when scanning ends), and
  `omr sheet render` writes PDF only.
- Phone photos: key sheets can be photographed (the instructor confirms them), but every answer
  on a photographed student sheet goes to review until the phone capture is calibrated.
- The project store is not encrypted at rest; keep exam folders on an encrypted university
  device.
- Every input file is read into memory for the run; files over 2 GB are refused.
- **Migration from Remark** (each with its workaround):
  - Only the graded Excel (`.xlsx`) Advanced Grade export is read; Remark's CSV and SPSS
    exports, version-mapping and scaled-score tables, and Remark-compatible exports are not
    built (spec §3.5, §16.3): export the Excel reports from Remark.
  - Remark's response data cannot become an assessment without the scans (TP-154 needs the
    exam graded here): scan the sheets.
  - AND keys (`A+C`) are refused: write `A|C` (either) or void the question.
  - A learning objective whose questions the students do not tell apart is refused, not split
    equally: give its points with `--points` (or *Use the suggested points*).
  - Imported CLOs land in domain K and their versions' objectives may differ: correct
    `clo.csv`, then `omr clo replace` and `omr clo approve` (or Setup's buttons).
  - A graded exam's points cannot be changed (its form map is stored once): create the exam
    from the import instead (`omr grade`, `omr key replace` and the desktop's *Replace key…*
    all read the key file's `points_A`/`points_B` columns).
  - The Setup grid's points column shows version A's points; a cell that stands for one
    version's rows is labelled "version A only".
  - Booklets are paired with Remark's rows by student number, then by answers (a heuristic
    whose every pairing is reported as an identity difference); an ID Excel already rounded
    (`2.2E+09`) cannot be recovered: export it again from Remark with the ID column as text.
  - The parallel run holds release only when its differences are queued (`--queue`, *Queue for
    review*); it is an internal check until the licence review (Q-08) is recorded.

The full list, with the evidence for each row: [`docs/EVIDENCE.md`](docs/EVIDENCE.md) §7.

## 9. For developers

```sh
scripts/check.sh        # formatting, clippy, every test, architecture rules
scripts/acceptance.sh   # the MVP acceptance rows A1–A11 in release; writes docs/evidence/acceptance.json
```

The crates and the engineering rules are in [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md)
§3–§4; each crate has its own README. The technical design
([`docs/TECHNICAL_DESIGN.md`](docs/TECHNICAL_DESIGN.md), Draft v0.2) covers the architecture,
recognition, identity, scoring and outcomes, audit, security and privacy, Saudi regulatory
obligations, quality assurance and the delivery plan.
