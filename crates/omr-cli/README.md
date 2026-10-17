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
| `omr sheet render --spec spec.json --out sheets.pdf [--copies N]` | Printable PDF of every form × N copies (serials 1…N). The spec is `{"template": TemplateSpec, "exam_key_hex": "…"}`; without a key a new 32-byte page-code key is generated and written next to the PDF as `<out>.exam-key.json` (keep it secret; grading needs it) |
| `omr grade <files/folders…> (--template NAME \| --spec spec.json) --questions N --out DIR` | The whole pipeline. Options: `--key key.csv` or `--infer-key`, `--roster roster.csv\|.xlsx`, `--clo clo.csv`, `--forms A,B`, `--options M`, `--cancel-mode erase_only\|cross_cancels\|fill_cancels_check`, `--multi-review never\|always\|high_stakes`, `--high-stakes`, `--id-scheme iau\|pnu\|ksu\|digits:N`, `--title`, `--course`, `--term`, `--threads N`, `--snapshot-time RFC3339`, `--operator`, `--quiet`. Writes the exports and `DIR/project.omrproj` (refuses an existing project) |
| `omr review list --project DIR [--lane marks\|identity\|capture\|scoring] [--json] [--crops PNGDIR]` | Open items in queue order (§13.2); `--crops` writes each item's crop as PNG |
| `omr review resolve --project DIR --item ID --decision D [--group]` | Records a decision as audited events and re-exports. `D`: `A`…, `1`…, Arabic letters, `blank`, `multi:A+C`, `unreadable`, `student:ID`, `not-on-roster`, `form:B`, `ack` |
| `omr review resolve --project DIR --item ID --defer \| --escalate \| --rescan` | Queue moves (events) |
| `omr review resolve --project DIR --undo` | Compensates the latest review event |
| `omr bench <corpus…> (--template \| --spec) --questions N [--threads 1,2,4] [--first 30] [--repeat R] [--out bench.json]` | A2: pages/s per thread count (recognition only, and the full in-memory pipeline); A3: the first `--first` sheets end to end with store and exports |

**Inputs.** Key CSV in the long layout `question,A,B[,points]` (one column per form; a single
`key` column applies to every form) or the wide layout `form,Q1,Q2,…`; entries `B`, `A|C`
(any of), `*` (void), empty (missing → SCO-003). Roster CSV/XLSX with a header row
(`student_id`/`ID`/`الرقم الجامعي`, `name`, `name_ar`, `section`). CLO CSV
`clo,domain,title[,title_ar][,plo],questions[,weight][,note]` with question lists like `1-5,9`.
Without `--forms`, the forms come from the key file, else `A,B` for legacy sheets and the
spec's forms for own sheets.

**Exit codes.** 0 success (open review items are not an error: the summary line and
`summary.json` say whether release is blocked); 2 usage or exam definition; 3 input (missing
file, unreadable key/roster, existing project); 4 project store; 5 scoring; 6 review decision
refused; 7 not found (no graded exam, unknown item); 8 I/O; 70 internal (a panic is reported
as 70, never Rust's 101). `--snapshot-time` is checked (RFC 3339 UTC) before any work, and
the review flags `--decision`, `--defer`, `--escalate`, `--rescan` and `--undo` exclude each
other (clap usage error, exit 2). A resolved item refuses `--defer`/`--rescan` (exit 6; use
`--undo`).

## Tests

`tests/cli.rs` (synthetic sheets from `omr-sim`, a dev-dependency): `templates`, `sheet render`,
`grade` with key, roster and CLO file plus a corrupt scan (CAP-001 in the Capture lane),
`review list --json`, `review resolve` and `--undo` (open count restored), a refused decision
(exit 6), `bench` (two thread counts, JSON report); exit codes 2/3/7 and clap usage errors.

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
