# omr-cli

The `omr` binary (spec §4.3, §16.4): a headless front end over `omr-app` for own-form sheets,
grading, review and benchmarks.

```sh
export CARGO_TARGET_DIR=$PWD/target-app
cargo run --release -p omr-cli -- --help
```

## Commands

| Command | What |
|---|---|
| `omr templates [--example-spec]` | Lists the legacy templates (`iau-ccsit-v1`) and how to use generated sheets; `--example-spec` prints a starting own-form spec (JSON) |
| `omr sheet render --spec spec.json --out sheets.pdf [--copies N] [--key-out FILE] [--new-key]` | Printable PDF of every form × N copies (serials 1…N). The spec is `{"template": TemplateSpec, "exam_key_hex": "…"}`. A bare spec gets a fresh exam UUID and a new 32-byte page-code key, written owner-only to `--key-out` or to the private key folder (`<key dir>/exam-keys/<exam>.json`), never beside the PDF that goes to the print shop; grade with `--spec` that file. An existing key file is refused (exit 3) unless `--new-key` (sheets printed with the old key then become foreign) |
| `omr grade <files/folders…> (--template NAME \| --spec spec.json) --questions N --out DIR` | The whole pipeline. Options: `--key key.csv` or `--infer-key`, `--key-sheet FILE` (repeatable: a teacher-filled key sheet, §10.1; with key sheets the run is held, nothing scored, until `omr key confirm`; key sheets inside the scans are found by the marker rule, `--key-sheet-marker 0,9\|none`), `--roster roster.csv\|.xlsx`, `--clo clo.csv`, `--forms A,B`, `--options M`, `--cancel-mode erase_only\|cross_cancels\|fill_cancels_check`, `--multi-review never\|always\|high_stakes`, `--high-stakes`, `--id-scheme iau\|pnu\|ksu\|digits:N`, `--title`, `--course`, `--term`, `--threads N`, `--snapshot-time RFC3339`, `--operator`, `--quiet`, `--reuse-exam-id`. `--id-scheme` defaults to `iau` on the IAU sheet and to the printed ID grid on own sheets. Writes the exports and `DIR/project.omrproj` (refuses an existing project, a sync or network folder (SYS-015), and a second project of the same own-sheet exam unless `--reuse-exam-id`); the folder is owner-only (0700, files 0600). Pages held out of grading (CAP-001, CAP-002, REG-001, IDN-002 …) are listed after the summary, each with its file, page, reason and what to do (R10); the last line names the next command (`omr review list` or `omr release`) |
| `omr add <files/folders…> --project DIR [--threads N] [--snapshot-time T] [--operator P] [--no-export]` | A later batch (another section, another day): the exam is graded again over every stored original plus the new files (identical to grading everything at once), the key and every review decision carry over (a contradicted human value → SYS-032), a re-imported file is CAP-011; prints the new batch, pages, booklets before → after, carried decisions and conflicts |
| `omr key read <key sheets…> (--template \| --spec) --questions N [--forms A,B] --out DIR [--key-sheet-marker 0,9]` | Reads teacher-filled key sheets on their own (every sheet given is a key sheet, §10.1) with the normal recognizer, in memory; writes `DIR/key.csv` (the `--key` layout, one column per version read; a row that is not one confidently read option is left empty, never guessed) and `DIR/key_summary.html` (per version, each question's read answer beside its rectified bubble-row crop, rows to check highlighted; each crop shows the question number printed beside its row). Exit 0 when every version read has every row; exit 5 naming each sheet refused (no marker: e.g. `1111111111` from a sheet registered one row off; not registered; version not bubbled) and each row to decide. The terminal shows counts only, never the key, and says it is a preview, not an audited confirmation |
| `omr key summary --project DIR [--out FILE]` | Writes the key summary of a graded exam's key sheets (default `key_sheet_summary.html` in the exam folder) and lists the questions that need a decision |
| `omr key confirm --project DIR [--version A] [--answer 7=B \| 7=A\|C \| 7=* \| A:7=B …] [--reason CODE [--note T]] [--operator P]` | The instructor confirms the key read from key sheets: refused (exit 6) while a row waits, naming each one; the last version confirmed publishes the key version (`KeyVersionPublished`: source `key_sheet`, each scan's SHA-384 and page, the operator), scores the exam and re-exports |
| `omr key replace --project DIR --key key.csv [--reason CODE [--note T]]` | The whole key (e.g. the real key after `--infer-key`: the SCO-003 `inferred_key` blocker clears); new key version, rescored, re-exported |
| `omr key replace --project DIR --from-key-sheets FILE…` | The audited way to take the key from key sheets after grading (`ExamProject::add_key_sheets`): the files join the exam as named key sheets (originals stored with the scans; a file already graded as a student is graded again as the key sheet), the run is held, the key summary written; `omr key confirm` then publishes the key naming them. Confirmation warnings name the version and p only (SCO-027), never the key's letter counts |
| `omr key set --project DIR [--form A] --question N --answer B [--reason CODE]` | One entry (a mis-key; `A\|C`, `*` void, `""` missing); new key version with its reason (default `KEY_ERROR`) |
| `omr release --project DIR [--reason CODE [--note T]] [--operator P]` | Releases the results (the instructor's act, §13.4): refused (exit 6) while anything blocks release, naming each reason and the next command; records `ResultsReleased` and writes the exports again with the Blackboard upload. A later review, key or batch change supersedes the release (`ReleaseSuperseded`) |
| `omr export --project DIR [--out DIR] [--blackboard-column ID]` | Every export again (the folder must not be a sync folder); the Blackboard upload only for released results; `--blackboard-column` writes it for that existing grade column (`blackboard_grade_center-column-ID.csv`) |
| `omr review list --project DIR [--lane marks\|identity\|capture\|scoring] [--json] [--reveal] [--crops PNGDIR]` | Open items in queue order (§13.2); a page-level item is followed by its file, page, reason and remedy. Student numbers are masked to their last 3 digits unless `--reveal` (recorded as `PersonalDataViewed`); `--crops` writes each item's crop as an owner-only PNG with a signed manifest, recorded as an export |
| `omr review resolve --project DIR --item ID --decision D [--group] [--reason CODE [--note T]]` | Records a decision as audited events and re-exports. `D`: `A`…, `1`…, Arabic letters, `blank`, `multi:A+C`, `unreadable`, `student:ID`, `not-on-roster`, `form:B`, `ack`, `void`. A probable mis-key (SCO-001, Scoring lane) is decided by a letter (change the key to it), `multi` (accept the key or the flagged option), `void`, or `ack --reason CODE --note T` (keep the key); each key change is an audited key version. A decision other than the engine's proposal needs `--reason` (a §14.4 code such as `AMBIGUOUS_MARK`, `ERASURE_RESIDUE`, `TEMPLATE_MISALIGN`, `ID_CORRECTION`, `DUPLICATE_RESOLVED`; `OTHER` needs `--note`), else exit 6 with the codes to choose from |
| `omr review resolve --project DIR --item ID --defer \| --escalate \| --rescan` | Queue moves (events) |
| `omr review resolve --project DIR --undo` | Compensates the latest review event |
| `omr bench <corpus…> (--template \| --spec) --questions N [--threads 1,2,4] [--first 30] [--repeat R] [--out bench.json]` | A2: pages/s per thread count (recognition only, and the full in-memory pipeline); A3: the first `--first` sheets end to end with store and exports |

**Inputs.** Key file (CSV, semicolon or tab separated; UTF-8 with or without BOM, UTF-16,
Windows-1256; CRLF or LF) in the long layout `question,A,B[,points]` (one column per form; a
single `key`/`answer` column applies to every form; headerless `1,B` or `1,B,C` too), the wide
layout `form,Q1,Q2,…` (or headerless rows starting with the form letter), or a single column
of answers; entries `B`/`b`, `A|C`, `A/C`, `AC`, `A or C` (any of), `*`/`void` (void), empty
(missing → SCO-003); question numbers `7`, `Q7`, `7.` or Arabic-Indic. A malformed line is
refused with its number and the fix (exit 3). Roster CSV/XLSX with a header row
(`student_id`/`ID`/`الرقم الجامعي`, `name`, `name_ar`/`الاسم العربي`, `section`), possibly
below title rows; Excel-damaged IDs are repaired when nothing was lost (`2210345601.0`,
full-precision `E+`, leading zeros under the ID scheme) and refused otherwise (`2.2E+09`);
duplicate IDs refuse the roster naming the rows (exit 3); warnings go to stderr. CLO CSV
`clo,domain,title[,title_ar][,plo],questions[,weight][,note]` with question lists like `1-5,9`.
Without `--forms`, the forms come from the key file, else `A,B` for legacy sheets and the
spec's forms for own sheets.

**Device key and checkpoints.** Every command that changes a project signs a checkpoint of
its audit chain with this computer's ECDSA P-384 key and keeps a copy outside the project, in
the private key folder (`$OMR_KEY_DIR`, else `$XDG_DATA_HOME/omr` or `~/.local/share/omr`;
`%APPDATA%\omr` on Windows). An older copy of a project put back in place, a truncated or
re-chained log, or a session snapshot edited outside the program is refused with SYS-011 and
what to do. `manifest.json` carries the signature of the exports.

**Exit codes.** 0 success (open review items are not an error: the summary line and
`summary.json` say whether release is blocked); 2 usage or exam definition; 3 input (missing
file, unreadable key/roster, existing project); 4 project store; 5 scoring; 6 review decision
refused; 7 not found (no graded exam, unknown item); 8 I/O; 70 internal (a panic is reported
as 70, never Rust's 101). `--snapshot-time` is checked (RFC 3339 UTC) before any work, and
the review flags `--decision`, `--defer`, `--escalate`, `--rescan` and `--undo` exclude each
other (clap usage error, exit 2). A resolved item refuses `--defer`/`--rescan` (exit 6; use
`--undo`).

## Tests

`tests/real_world.rs` (synthetic `omr-sim` sheets): a roster with a duplicate ID refused before
grading; day 1 graded with `--infer-key`, day 2 added with `omr add` (booklets 4 → 8), a day-1
file added again (CAP-011, nothing added); `omr key replace` (outputs no longer INFERRED) and
`omr key set --reason KEY_ERROR` (next key version; a malformed answer → exit 6 with the fix);
an override without `--reason` → exit 6 listing codes, `OTHER` without `--note` refused, with
both recorded; `omr release` refused while items are open (exit 6, naming `omr review list`),
then every item resolved and the exam released with its Blackboard upload;
`omr export --blackboard-column` (Username and score only; a malformed column ID → exit 3).

`tests/key_sheets.rs` (synthetic IAU key sheets on the committed blank form): `key read` writes
the expected `key.csv` and the summary; a multi-mark row is left empty and named (exit 5), a key
sheet reading `1111111111` is refused; `grade --key-sheet` is held (no `results.csv`, `release`
and `key set` exit 6, `review list` names the hold), `key confirm` refuses the undecided row,
then `--answer A:3=…` confirms: key version 1, `key.csv` equals the key, results written.
`tests/real_key_read.rs` (skipped unless `OMR_IAU_PDF_DIR` and `OMR_IAU_KEYS` are set): `key
read` on the instructors' real key sheets; counts only (§4 of `docs/EVIDENCE.md`).

`tests/acceptance_perf.rs` (A2, A3; `#[ignore]`d timing runs, executed in release one at a time
by `scripts/acceptance.sh`): `omr bench` at 1, 2 and 4 threads (best of 3) on 240 own-form
200-dpi JPEG sheets (`omr-sim`) and on 150 synthetic legacy IAU sheets delivered as 600-dpi
bitonal PDFs of 30 pages (the committed blank form with painted marks, shared with omr-app's
`tests/common`), asserting ≥ 40 recognition pages/s at 4 threads; and the wall time of the
`omr grade` process on exactly 30 sheets of each corpus with key, roster and CLO file (< 3 s).

`tests/cli.rs` (synthetic sheets from `omr-sim`, a dev-dependency): `templates`, `sheet render`,
`grade` with key, roster and CLO file plus a corrupt scan (CAP-001 in the Capture lane, and
the summary names the file, the reason and the remedy),
`review list --json`, `review resolve` and `--undo` (open count restored), a refused decision
(exit 6), `bench` (two thread counts, JSON report); exit codes 2/3/7 and clap usage errors;
`own_form_first_use_with_default_flags`: `templates --example-spec` (a fresh exam UUID each
run), `sheet render` keeping the key in the private key folder (a second render refused
without `--new-key`), grading with the default ID scheme, and a second project of the same
exam refused unless `--reuse-exam-id`. Each test uses its own `OMR_KEY_DIR`.

## Real-data runs (not committed)

```sh
omr grade <uploads>/…Full_Batch_A.pdf <uploads>/…Full_Batch_B.pdf --template iau-ccsit-v1 \
    --questions 29 --infer-key --out <scratch>/e2e/cs522
omr grade <uploads>/…ARTI503…pdf (3 files) --template iau-ccsit-v1 --questions 28 \
    --infer-key --out <scratch>/e2e/arti503
omr bench <uploads> --template iau-ccsit-v1 --questions 29 --threads 1,2,4 --first 30 \
    --scratch <scratch>/bench   # the A3 run spools the originals: keep it in scratch
```

Results are in `crates/omr-app/README.md` (aggregates only).
