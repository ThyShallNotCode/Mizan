//! `omr`: headless command-line front end (spec §4.3, §16.4).
//!
//! ```text
//! omr templates
//! omr sheet render --spec spec.json --out sheets.pdf [--copies N]
//! omr grade <inputs…> --template iau-ccsit-v1 | --spec spec.json --questions N
//!           [--key key.csv | --infer-key] [--roster roster.csv|.xlsx] [--clo clo.csv] --out DIR
//! omr add --project DIR <more inputs…>        (a later batch; review decisions carry over)
//! omr review list --project DIR [--lane marks] [--json]
//! omr review resolve --project DIR --item ID (--decision B [--reason CODE [--note T]] | --defer
//!                    | --escalate | --rescan | --undo)
//! omr key replace --project DIR --key key.csv [--reason CODE]
//! omr key set --project DIR [--form A] --question N --answer B [--reason CODE]
//! omr release --project DIR [--reason CODE [--note T]]
//! omr export --project DIR [--out DIR] [--blackboard-column ID]
//! omr bench <corpus-dir> --template … --questions N [--threads 1,2,4] [--first 30]
//! ```
//!
//! Exit codes: 0 success; 2 usage or exam definition; 3 input; 4 project store; 5 scoring;
//! 6 review; 7 not found; 8 I/O; 70 internal.
#![forbid(unsafe_code)]

use clap::{Args, Parser, Subcommand};
use omr_app::exam::{self, IdSchemeChoice};
use omr_app::review::ReviewReason;
use omr_app::{
    Action, AppError, Decision, ExamDefinition, ExamProject, GradeOptions, KeySource, Lane,
    MultiReviewPolicy, Progress, TemplateChoice,
};
use omr_forms::{ExamKey, SheetInstance, Template, TemplateSpec};
use omr_model::{BookletSerial, CancelMode, FormCode, PageNo};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "omr",
    version,
    about = "OMR Correction: sheets, grading, review and benchmarks"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the answer-sheet templates.
    Templates {
        /// Print an example own-form spec (JSON) to start from.
        #[arg(long)]
        example_spec: bool,
    },
    /// Own-form sheets.
    Sheet {
        #[command(subcommand)]
        cmd: SheetCmd,
    },
    /// Grade scans: recognition, identity, scoring, outcomes, exports.
    Grade(GradeArgs),
    /// Add a later batch of scans to a graded exam (e.g. a second section scanned on another
    /// day); the exam is regraded with every earlier review decision kept.
    Add(AddArgs),
    /// Review queue of a graded exam.
    Review {
        #[command(subcommand)]
        cmd: ReviewCmd,
    },
    /// Answer key of a graded exam: replace it (e.g. the real key after `--infer-key`) or
    /// correct one entry; an audited regrade with a new key version.
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// Throughput (A2) and time to first result (A3) on a corpus folder.
    Bench(BenchArgs),
    /// Releases the results of a graded exam (the instructor's deliberate act, §13.4): refused
    /// while anything blocks release; then the exports are written again with the Blackboard
    /// upload.
    Release {
        /// Graded output folder or its `project.omrproj`.
        #[arg(long)]
        project: PathBuf,
        /// Reason code recorded on the release (optional).
        #[arg(long)]
        reason: Option<String>,
        /// Note for the reason.
        #[arg(long, requires = "reason")]
        note: Option<String>,
        /// Acting principal (the instructor).
        #[arg(long, default_value = "principal:instructor")]
        operator: String,
    },
    /// Writes every export of a graded exam again (results, reports, Blackboard upload).
    Export {
        /// Graded output folder or its `project.omrproj`.
        #[arg(long)]
        project: PathBuf,
        /// Export folder (default: the folder holding the project).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Blackboard grade-column ID (the number after `|` in a downloaded column header) to
        /// update an existing column instead of creating one.
        #[arg(long)]
        blackboard_column: Option<String>,
    },
}

#[derive(Subcommand)]
enum SheetCmd {
    /// Renders printable sheets (PDF) from an own-form spec.
    Render {
        /// Spec JSON: `{"template": TemplateSpec, "exam_key_hex": "…"}` or a bare TemplateSpec.
        #[arg(long)]
        spec: PathBuf,
        /// Output PDF.
        #[arg(long)]
        out: PathBuf,
        /// Copies per form (serials 1…N).
        #[arg(long, default_value_t = 1)]
        copies: u32,
        /// Where a bare spec's new exam key is written (default: the private key folder,
        /// `$OMR_KEY_DIR` or `~/.local/share/omr/exam-keys/`).
        #[arg(long)]
        key_out: Option<PathBuf>,
        /// Replace an existing key file at --key-out (sheets printed with it become foreign).
        #[arg(long)]
        new_key: bool,
    },
}

#[derive(Args, Clone)]
struct ExamArgs {
    /// Legacy template name (see `omr templates`).
    #[arg(long, conflicts_with = "spec")]
    template: Option<String>,
    /// Own-form spec JSON (with its page-code key).
    #[arg(long)]
    spec: Option<PathBuf>,
    /// Questions used by the exam.
    #[arg(long)]
    questions: u16,
    /// Options per question (default: the sheet's).
    #[arg(long)]
    options: Option<u8>,
    /// Form (version) letters, comma separated (default: from the key file, else A,B for
    /// legacy sheets and the spec's forms for own sheets).
    #[arg(long, value_delimiter = ',')]
    forms: Option<Vec<String>>,
    /// Marking convention: erase_only | cross_cancels | fill_cancels_check.
    #[arg(long, default_value = "erase_only")]
    cancel_mode: String,
    /// Confident multi marks: never | always | high_stakes.
    #[arg(long, default_value = "never")]
    multi_review: String,
    /// High-stakes mode.
    #[arg(long)]
    high_stakes: bool,
    /// Student-ID scheme: iau | pnu | ksu | digits:N (default: iau on IAU sheets; on own
    /// sheets the printed ID grid's, iau for 10 digits else digits:N).
    #[arg(long)]
    id_scheme: Option<String>,
    /// Exam title.
    #[arg(long, default_value = "Exam")]
    title: String,
    /// Course code.
    #[arg(long, default_value = "")]
    course: String,
    /// Term label.
    #[arg(long, default_value = "")]
    term: String,
}

#[derive(Args)]
struct GradeArgs {
    /// Scan files and folders (PDF, JPEG, PNG, TIFF).
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    #[command(flatten)]
    exam: ExamArgs,
    /// Answer key CSV (`question,A,B[,points]` or `form,Q1,Q2,…`).
    #[arg(long, conflicts_with = "infer_key")]
    key: Option<PathBuf>,
    /// Infer each form's key from the modal confident responses (never releasable).
    #[arg(long)]
    infer_key: bool,
    /// Roster CSV or XLSX (student_id, name, section).
    #[arg(long)]
    roster: Option<PathBuf>,
    /// CLO file CSV (clo, domain, title, plo, questions, weight, note).
    #[arg(long)]
    clo: Option<PathBuf>,
    /// Output folder (exports + `project.omrproj`).
    #[arg(long)]
    out: PathBuf,
    /// Worker threads (0 = all cores).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Report snapshot time (RFC 3339 UTC) for reproducible outputs.
    #[arg(long)]
    snapshot_time: Option<String>,
    /// Operator principal.
    #[arg(long, default_value = "principal:operator")]
    operator: String,
    /// No progress lines.
    #[arg(long)]
    quiet: bool,
    /// Grade own sheets whose exam was already graded into another project (the same exam
    /// again, on purpose).
    #[arg(long)]
    reuse_exam_id: bool,
}

#[derive(Args)]
struct AddArgs {
    /// Scan files and folders of the new batch (PDF, JPEG, PNG, TIFF).
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Graded output folder or its `project.omrproj`.
    #[arg(long)]
    project: PathBuf,
    /// Worker threads (0 = all cores).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Report snapshot time (RFC 3339 UTC); default: the exam's.
    #[arg(long)]
    snapshot_time: Option<String>,
    /// Operator principal.
    #[arg(long, default_value = "principal:operator")]
    operator: String,
    /// Skip re-exporting.
    #[arg(long)]
    no_export: bool,
}

#[derive(Subcommand)]
enum ReviewCmd {
    /// Lists open review items in queue order.
    List {
        /// Graded output folder or its `project.omrproj`.
        #[arg(long)]
        project: PathBuf,
        /// Lane: marks | identity | capture | scoring.
        #[arg(long)]
        lane: Option<String>,
        /// JSON output.
        #[arg(long)]
        json: bool,
        /// Export each item's crop as PNG into this folder (a Restricted export: private
        /// files, a signed manifest, recorded in the audit log).
        #[arg(long)]
        crops: Option<PathBuf>,
        /// Show student numbers in full (masked to their last 3 digits by default); the
        /// reveal is recorded in the audit log.
        #[arg(long)]
        reveal: bool,
    },
    /// Records a review decision (an audited event) and re-exports.
    Resolve {
        /// Graded output folder or its `project.omrproj`.
        #[arg(long)]
        project: PathBuf,
        /// Item ID (from `review list`); not needed for `--undo`.
        #[arg(long)]
        item: Option<u64>,
        /// A, B…, blank, multi:A+C, unreadable, student:ID, not-on-roster, form:B, ack.
        #[arg(long)]
        decision: Option<String>,
        /// Apply the decision to the item's whole group.
        #[arg(long, requires = "decision")]
        group: bool,
        /// Defer to the end of the queue.
        #[arg(long, conflicts_with_all = ["decision", "escalate", "rescan", "undo"])]
        defer: bool,
        /// Escalate to the instructor.
        #[arg(long, conflicts_with_all = ["decision", "rescan", "undo"])]
        escalate: bool,
        /// Add to the rescan pull list.
        #[arg(long, conflicts_with_all = ["decision", "undo"])]
        rescan: bool,
        /// Undo the latest review event.
        #[arg(long, conflicts_with_all = ["decision", "item"])]
        undo: bool,
        /// Reason code (§14.4), required when the decision overrides the engine's proposal,
        /// e.g. AMBIGUOUS_MARK, ERASURE_RESIDUE, TEMPLATE_MISALIGN, ID_CORRECTION, OTHER.
        #[arg(long)]
        reason: Option<String>,
        /// Note for the reason (required with OTHER).
        #[arg(long, requires = "reason")]
        note: Option<String>,
        /// Reviewer principal.
        #[arg(long, default_value = "principal:reviewer")]
        reviewer: String,
        /// Skip re-exporting.
        #[arg(long)]
        no_export: bool,
    },
}

#[derive(Subcommand)]
enum KeyCmd {
    /// Replaces the whole key from a key file (same layouts as `grade --key`).
    Replace {
        /// Graded output folder or its `project.omrproj`.
        #[arg(long)]
        project: PathBuf,
        /// Answer key CSV.
        #[arg(long)]
        key: PathBuf,
        /// Reason code (default KEY_ERROR).
        #[arg(long)]
        reason: Option<String>,
        /// Note for the reason.
        #[arg(long, requires = "reason")]
        note: Option<String>,
        /// Acting principal.
        #[arg(long, default_value = "principal:instructor")]
        operator: String,
        /// Skip re-exporting.
        #[arg(long)]
        no_export: bool,
    },
    /// Sets one key entry (a mis-key correction).
    Set {
        /// Graded output folder or its `project.omrproj`.
        #[arg(long)]
        project: PathBuf,
        /// Form letter (default: the first form).
        #[arg(long)]
        form: Option<String>,
        /// Question number (1-based, as printed).
        #[arg(long)]
        question: u32,
        /// B, A|C (several accepted), * (void), or "" (missing).
        #[arg(long, allow_hyphen_values = true)]
        answer: String,
        /// Reason code (default KEY_ERROR).
        #[arg(long)]
        reason: Option<String>,
        /// Note for the reason.
        #[arg(long, requires = "reason")]
        note: Option<String>,
        /// Acting principal.
        #[arg(long, default_value = "principal:instructor")]
        operator: String,
        /// Skip re-exporting.
        #[arg(long)]
        no_export: bool,
    },
}

#[derive(Args)]
struct BenchArgs {
    /// Corpus folder (or files).
    #[arg(required = true)]
    corpus: Vec<PathBuf>,
    #[command(flatten)]
    exam: ExamArgs,
    /// Thread counts.
    #[arg(long, value_delimiter = ',', default_value = "1,2,4")]
    threads: Vec<usize>,
    /// Sheets for the time-to-first-result run.
    #[arg(long, default_value_t = 30)]
    first: usize,
    /// Repeats per thread count (best is reported).
    #[arg(long, default_value_t = 1)]
    repeat: usize,
    /// Scratch folder for the end-to-end runs (a store is written there).
    #[arg(long)]
    scratch: Option<PathBuf>,
    /// Write the JSON report here.
    #[arg(long)]
    out: Option<PathBuf>,
}

fn usage(m: impl Into<String>) -> AppError {
    AppError::Definition(m.into())
}

/// Spec file: `{ "template": TemplateSpec, "exam_key_hex": "…" }` or a bare TemplateSpec.
fn read_spec(path: &Path) -> Result<(TemplateSpec, Option<String>), AppError> {
    let bytes =
        std::fs::read(path).map_err(|e| AppError::Input(format!("{}: {e}", path.display())))?;
    let v: Value = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Input(format!("{}: not JSON: {e}", path.display())))?;
    let (t, key) = match v.get("template") {
        Some(t) => (
            t.clone(),
            v.get("exam_key_hex")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
        None => (v, None),
    };
    let spec: TemplateSpec = serde_json::from_value(t)
        .map_err(|e| AppError::Input(format!("{}: not a sheet spec: {e}", path.display())))?;
    Ok((spec, key))
}

fn cancel_mode(s: &str) -> Result<CancelMode, AppError> {
    match s {
        "erase_only" => Ok(CancelMode::EraseOnly),
        "cross_cancels" => Ok(CancelMode::CrossCancels),
        "fill_cancels_check" => Ok(CancelMode::FillCancelsCheck),
        o => Err(usage(format!(
            "--cancel-mode {o}: use erase_only, cross_cancels or fill_cancels_check"
        ))),
    }
}

fn definition(a: &ExamArgs, key_forms: Option<Vec<String>>) -> Result<ExamDefinition, AppError> {
    let cm = cancel_mode(&a.cancel_mode)?;
    let (template, options, default_forms, key_hex) = match (&a.template, &a.spec) {
        (Some(t), None) => (
            TemplateChoice::Legacy(t.clone()),
            5,
            vec!["A".to_string(), "B".to_string()],
            None,
        ),
        (None, Some(p)) => {
            let (spec, key) = read_spec(p)?;
            let opts = spec.options.max();
            let forms = (1..=spec.forms)
                .map(|f| FormCode(f).letter().to_string())
                .collect();
            (TemplateChoice::Generated(Box::new(spec)), opts, forms, key)
        }
        _ => return Err(usage("give --template NAME or --spec spec.json")),
    };
    let forms = a
        .forms
        .clone()
        .or(key_forms)
        .unwrap_or(default_forms)
        .into_iter()
        .map(|f| f.trim().to_uppercase())
        .collect();
    let mut d = ExamDefinition::new(
        &a.title,
        template,
        a.questions,
        a.options.unwrap_or(options),
        forms,
        cm,
    );
    d.course_code = a.course.clone();
    d.term = a.term.clone();
    d.multi_review = match a.multi_review.as_str() {
        "never" => MultiReviewPolicy::Never,
        "always" => MultiReviewPolicy::Always,
        "high_stakes" => MultiReviewPolicy::HighStakes,
        o => {
            return Err(usage(format!(
                "--multi-review {o}: use never, always or high_stakes"
            )));
        }
    };
    d.high_stakes = a.high_stakes;
    d.id_scheme = match a.id_scheme.as_deref() {
        // Own sheets: the scheme of the printed ID grid, so the default never disagrees with
        // the sheet (a 9-digit grid read under the 10-digit IAU scheme fails every booklet).
        None => match &d.template {
            TemplateChoice::Generated(spec) if spec.id_digits > 0 => {
                IdSchemeChoice::for_grid(usize::from(spec.id_digits))
            }
            _ => IdSchemeChoice::Iau,
        },
        Some("iau") => IdSchemeChoice::Iau,
        Some("pnu") => IdSchemeChoice::Pnu,
        Some("ksu") => IdSchemeChoice::Ksu,
        Some(s) => match s.strip_prefix("digits:").and_then(|n| n.parse().ok()) {
            Some(n) => IdSchemeChoice::Digits(n),
            None => {
                return Err(usage(format!(
                    "--id-scheme {s}: use iau, pnu, ksu or digits:N"
                )));
            }
        },
    };
    d.page_code_key_hex = key_hex;
    d.exam_id = d.derived_exam_id();
    Ok(d)
}

fn read_file(p: &Path) -> Result<Vec<u8>, AppError> {
    std::fs::read(p).map_err(|e| AppError::Input(format!("{}: {e}", p.display())))
}

fn key_forms_of(path: &Path) -> Option<Vec<String>> {
    exam::key_file_forms(&std::fs::read(path).ok()?)
}

fn reason_of(code: Option<&str>, note: Option<&str>) -> Option<ReviewReason> {
    code.map(|c| ReviewReason::with_note(c, note.unwrap_or("")))
}

fn cmd_templates(example: bool) -> Result<(), AppError> {
    if example {
        // A fresh exam ID every time (SEC-04: two exams never share one), and the IAU
        // 10-digit student number grid.
        let mut spec = TemplateSpec::standard(
            omr_model::ExamId(uuid::Uuid::now_v7()),
            40,
            4,
            CancelMode::EraseOnly,
        );
        spec.id_digits = 10;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"template": spec}))
                .map_err(|e| AppError::Internal(e.to_string()))?
        );
        return Ok(());
    }
    println!("Legacy templates (no fiducials, §6.10):");
    for (n, d) in exam::LEGACY_TEMPLATES {
        println!("  {n:<14} {d}");
    }
    println!("Generated sheets (own forms, §6.1): --spec spec.json");
    println!("  A4, AprilTag 36h11 corners, QR page code, row ticks; 2–10 options; 1–N forms.");
    println!("  `omr templates --example-spec > spec.json` prints a starting spec.");
    Ok(())
}

/// Where `sheet render` keeps a new exam's spec and page-code key: `--key-out`, else the
/// device's private key folder (never beside the PDF, which goes to the print shop).
fn key_file_path(key_out: Option<&Path>, exam: &omr_model::ExamId) -> Result<PathBuf, AppError> {
    if let Some(p) = key_out {
        return Ok(p.to_path_buf());
    }
    let dir = omr_app::keys::key_dir().ok_or_else(|| {
        usage("no private folder for the exam key (HOME is not set): pass --key-out PATH")
    })?;
    Ok(dir.join("exam-keys").join(format!("{exam}.json")))
}

fn cmd_render(
    spec: &Path,
    out: &Path,
    copies: u32,
    key_out: Option<&Path>,
    new_key: bool,
) -> Result<(), AppError> {
    let (mut spec, key_hex) = read_spec(spec)?;
    let key_hex = match key_hex {
        // The spec carries its key: the same exam (e.g. more copies).
        Some(k) => k,
        // A bare spec: a new exam, with a fresh exam ID and page-code key (SEC-04), kept in
        // a private file that is never overwritten (printed sheets would become foreign).
        None => {
            spec.exam = omr_model::ExamId(uuid::Uuid::now_v7());
            let mut k = [0u8; 32];
            rand::fill(&mut k);
            let hexk = hex::encode(k);
            let kp = key_file_path(key_out, &spec.exam)?;
            if kp.exists() {
                if !new_key {
                    return Err(AppError::Input(format!(
                        "{} already holds an exam key: sheets printed with it would no longer \
                         be accepted if it were replaced. Render more copies with `--spec {}`, \
                         or pass --new-key to replace it on purpose",
                        kp.display(),
                        kp.display()
                    )));
                }
                eprintln!(
                    "replacing {}: sheets printed with the old key are now foreign (IDN-002)",
                    kp.display()
                );
                std::fs::remove_file(&kp)?;
            }
            if let Some(dir) = kp.parent().filter(|d| !d.as_os_str().is_empty()) {
                omr_app::keys::create_private_dir(dir)?;
            }
            omr_app::keys::write_private_new(
                &kp,
                &serde_json::to_vec_pretty(&json!({"template": spec, "exam_key_hex": hexk}))
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )?;
            eprintln!(
                "new exam {}: its sheet spec and page-code key are in {} (private: anyone with \
                 it can print sheets this exam accepts); grade with `--spec {}`",
                spec.exam,
                kp.display(),
                kp.display()
            );
            hexk
        }
    };
    let kb: [u8; 32] = hex::decode(key_hex.trim())
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| usage("exam_key_hex must be 64 hex characters"))?;
    let key = ExamKey::from_bytes(kb);
    let t = Template::build(spec).map_err(|e| usage(format!("sheet spec: {e}")))?;
    let mut inst = Vec::new();
    for f in 1..=t.spec().forms {
        for c in 1..=copies {
            for p in 1..=t.page_count() {
                inst.push(
                    SheetInstance::new(&t, &key, FormCode(f), PageNo(p))
                        .with_serial(BookletSerial(c)),
                );
            }
        }
    }
    let pdf = omr_forms::render::sheets_pdf(&inst, &t.spec().title)
        .map_err(|e| AppError::Internal(format!("render: {e}")))?;
    std::fs::write(out, pdf)?;
    println!(
        "{}: {} page(s), {} form(s) × {copies} cop(y/ies), template sha384 {}…",
        out.display(),
        inst.len(),
        t.spec().forms,
        &t.hash_hex()[..16]
    );
    Ok(())
}

fn build_def(g: &GradeArgs) -> Result<ExamDefinition, AppError> {
    let key_forms = g.key.as_deref().and_then(key_forms_of);
    let mut d = definition(&g.exam, key_forms)?;
    if let Some(k) = &g.key {
        let (forms, points) =
            exam::read_key_file(&read_file(k)?, &d.forms, d.questions, d.options)?;
        d.key = KeySource::Provided { forms };
        if let Some(p) = points {
            d.points = p;
        }
    } else if g.infer_key {
        d.key = KeySource::Inferred;
    }
    if let Some(r) = &g.roster {
        let bytes = read_file(r)?;
        let ext = r
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "xlsm" || ext == "xls" {
            return Err(AppError::Input(format!(
                "{}: .{ext} workbooks are not read; save the roster as .xlsx or CSV UTF-8",
                r.display()
            )));
        }
        let rows = if ext == "xlsx" {
            exam::read_roster_xlsx(&bytes)?
        } else {
            exam::read_roster_csv(&bytes)?
        };
        // Student numbers are masked on the terminal (logs keep no full number, SEC-16).
        for w in d.import_roster(rows)? {
            eprintln!("roster: {}", omr_app::export::mask_numbers(&w.to_string()));
        }
    }
    if let Some(c) = &g.clo {
        d.outcomes = Some(exam::read_clo_csv(&read_file(c)?, d.questions)?);
    }
    d.validate()?;
    Ok(d)
}

fn project_dir(p: &Path) -> PathBuf {
    if p.join("project.omr").exists() || p.extension().is_some_and(|e| e == "omrproj") {
        p.to_path_buf()
    } else {
        p.join("project.omrproj")
    }
}

fn cmd_grade(g: GradeArgs) -> Result<(), AppError> {
    let def = build_def(&g)?;
    if let Some(t) = &g.snapshot_time {
        omr_app::pipeline::check_snapshot_utc(t)?;
    }
    omr_app::keys::export_dir(&g.out)?;
    let proj = g.out.join("project.omrproj");
    if proj.exists() {
        return Err(AppError::Input(
            if proj.join("cache").join("session.json").exists() {
                format!(
                    "{} already holds a graded exam; use a new --out folder (or `omr add --project \
                 {}` to add scans to it)",
                    g.out.display(),
                    g.out.display()
                )
            } else {
                format!(
                    "{} holds the start of a grading run that did not finish; delete {} and grade \
                 again",
                    g.out.display(),
                    proj.display()
                )
            },
        ));
    }
    // Own sheets name their exam in the page code: a second project for the same exam (its
    // spec and key reused for another sitting) would accept the first one's sheets (SEC-04).
    let own = matches!(def.template, TemplateChoice::Generated(_));
    if own
        && !g.reuse_exam_id
        && let Some(other) = omr_app::keys::exam_claimed_by(&def.exam_id, &proj)
    {
        return Err(AppError::Definition(format!(
            "the sheets of exam {} were already graded into {}: add these scans to that exam \
             (`omr add --project {}`), or render new sheets from a spec without a key for a new \
             exam (`omr sheet render`). Pass --reuse-exam-id only to grade the same exam again \
             on purpose",
            def.exam_id,
            other.display(),
            other.display()
        )));
    }
    let opts = GradeOptions {
        threads: g.threads,
        project_dir: Some(proj),
        snapshot_utc: g.snapshot_time.clone(),
        operator: g.operator.clone(),
        max_pages: None,
    };
    let quiet = g.quiet;
    let progress = move |p: &Progress| {
        if quiet {
            return;
        }
        match p {
            Progress::Started { pages, workers } => {
                eprintln!("grading {pages} page(s) on {workers} worker(s)")
            }
            Progress::Page {
                done, total, page, ..
            } if *done == *total || *done % 25 == 0 => {
                eprintln!(
                    "  {done}/{total} pages ({} {}/{} auto)",
                    page.page_id, page.auto_accepted, page.questions
                )
            }
            Progress::Stage(s) => eprintln!("  stage: {s}"),
            _ => {}
        }
    };
    let t0 = Instant::now();
    let mut p = ExamProject::grade(&def, &g.inputs, &opts, &progress)?;
    if own {
        omr_app::keys::claim_exam(&def.exam_id, &g.out.join("project.omrproj"))?;
    }
    let sum = p.export(&g.out)?;
    let s = &p.session;
    let t = &s.timings;
    let q_total: usize = s.booklets.iter().map(|b| b.questions.len()).sum();
    let q_auto: usize = s
        .booklets
        .iter()
        .flat_map(|b| &b.questions)
        .filter(|q| q.status == omr_app::model::ResponseStatus::Auto)
        .count();
    println!(
        "{} page(s) → {} booklet(s); {q_auto}/{q_total} answers auto-accepted (AR {:.2} %); {} open review item(s)",
        s.pages.len(),
        s.booklets.len(),
        if q_total == 0 {
            0.0
        } else {
            100.0 * (q_total - q_auto) as f64 / q_total as f64
        },
        p.queue(None).len()
    );
    println!(
        "wall {:.0} ms (pages {:.0} ms, {:.1} pages/s at {} thread(s)); first result {:.0} ms",
        t0.elapsed().as_secs_f64() * 1e3,
        t.page_stage_ms,
        if t.page_stage_ms > 0.0 {
            t.pages as f64 / (t.page_stage_ms / 1e3)
        } else {
            0.0
        },
        t.threads,
        t.first_result_ms
    );
    match s.release_blocked() {
        Some(why) => println!(
            "release blocked: {why}; next: omr review list --project {}",
            shell_arg(&g.out)
        ),
        None => println!(
            "ready to release: omr release --project {}",
            shell_arg(&g.out)
        ),
    }
    print_page_problems(&p);
    println!("wrote {} file(s) to {}", sum.files.len(), g.out.display());
    p.close()
}

/// Pages held out of grading (capture, registration, foreign sheets), each with its file, the
/// reason and what to do (R10).
fn print_page_problems(p: &ExamProject) {
    let items: Vec<_> = p
        .queue(Some(Lane::Capture))
        .into_iter()
        .filter(|i| i.page.is_some())
        .collect();
    if items.is_empty() {
        return;
    }
    println!(
        "{} page(s) not graded (omr review list --lane capture):",
        items.len()
    );
    for it in items.iter().take(20) {
        println!(
            "  {} {}: {}",
            it.code,
            format!("{:?}", it.severity).to_uppercase(),
            it.detail
        );
    }
    if items.len() > 20 {
        println!("  … and {} more", items.len() - 20);
    }
}

fn cmd_review(cmd: ReviewCmd) -> Result<(), AppError> {
    match cmd {
        ReviewCmd::List {
            project,
            lane,
            json: as_json,
            crops,
            reveal,
        } => {
            let mut p = ExamProject::open(&project_dir(&project), "principal:viewer")?;
            let lane = match lane.as_deref() {
                None => None,
                Some(l) => Some(Lane::parse(l).ok_or_else(|| {
                    usage(format!(
                        "--lane {l}: use marks, identity, capture or scoring"
                    ))
                })?),
            };
            let ids: Vec<u64> = p.queue(lane).iter().map(|i| i.id).collect();
            if let Some(dir) = &crops {
                let s = p.export_crops(dir, &ids)?;
                eprintln!(
                    "exported {} crop(s) and a signed manifest to {} (Restricted: keep them on \
                     university devices)",
                    s.files.len() - 1,
                    dir.display()
                );
            }
            if reveal {
                p.record_reveal("review-list", ids.len())?;
            }
            let shown = |t: String| {
                if reveal {
                    t
                } else {
                    omr_app::export::mask_numbers(&t)
                }
            };
            let items = p.queue(lane);
            if as_json {
                let mut v = omr_app::export::review_json(&p);
                if !reveal {
                    omr_app::export::mask_json(&mut v);
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&v)
                        .map_err(|e| AppError::Internal(e.to_string()))?
                );
            } else {
                let labels = p.session.definition.option_labels();
                println!("{} open item(s)", items.len());
                for it in items {
                    let b = it.booklet.map(|b| &p.session.booklets[b]);
                    println!(
                        "{:>5}  {:<8} {:<7} {:<8} {:<16} {:<12} {:<10} {}",
                        it.id,
                        it.lane.label(),
                        it.code,
                        format!("{:?}", it.severity).to_uppercase(),
                        it.sub_reason.clone().unwrap_or_default(),
                        b.map_or_else(String::new, |b| b.label()),
                        it.question
                            .map_or_else(String::new, |q| format!("Q{}", q + 1)),
                        shown(it.suggestion.as_ref().map_or_else(String::new, |d| format!(
                            "suggest {}",
                            d.text(&labels, &p.session.definition.forms)
                        )))
                    );
                    // Page-level items carry the file, the reason and what to do (R10).
                    if it.kind == omr_app::ItemKind::Notice && it.page.is_some() {
                        println!("       {}", shown(it.detail.clone()));
                    }
                }
            }
            Ok(())
        }
        ReviewCmd::Resolve {
            project,
            item,
            decision,
            group,
            defer,
            escalate,
            rescan,
            undo,
            reason,
            note,
            reviewer,
            no_export,
        } => {
            let dir = project_dir(&project);
            let mut p = ExamProject::open(&dir, &reviewer)?;
            p.set_actor(&reviewer, "Reviewer");
            let reason = reason_of(reason.as_deref(), note.as_deref());
            if undo {
                match p.undo()? {
                    Some(id) => println!("undid the latest event; item {id} re-opened"),
                    None => println!("nothing to undo"),
                }
            } else {
                let id = item.ok_or_else(|| usage("--item ID is required"))?;
                let def = &p.session.definition;
                let action = if defer {
                    Action::Defer
                } else if escalate {
                    Action::Escalate
                } else if rescan {
                    Action::AddToRescan
                } else {
                    let d = decision.as_deref().ok_or_else(|| {
                        usage("give --decision, --defer, --escalate, --rescan or --undo")
                    })?;
                    let d = Decision::parse(d, def.options, &def.forms)
                        .ok_or_else(|| usage(format!("--decision {d}: not understood")))?;
                    if group {
                        Action::ApplyToGroup { decision: d }
                    } else {
                        Action::Resolve {
                            decision: d,
                            accepted_suggestion: false,
                            note: String::new(),
                        }
                    }
                };
                let out = p.apply_with_reason(id, action, reason)?;
                println!(
                    "recorded; affected {:?}; next {}",
                    out.affected,
                    out.next.map_or("none".into(), |n| n.to_string())
                );
            }
            if !no_export {
                reexport(&mut p, &dir)?;
            }
            println!("{} open item(s) remain", p.queue(None).len());
            p.close()
        }
    }
}

/// Re-exports a project into the folder that holds its package.
fn reexport(p: &mut ExamProject, dir: &Path) -> Result<(), AppError> {
    let out = dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dir.to_path_buf());
    let s = p.export(&out)?;
    println!("re-exported {} file(s) to {}", s.files.len(), out.display());
    Ok(())
}

fn cmd_add(a: AddArgs) -> Result<(), AppError> {
    if let Some(t) = &a.snapshot_time {
        omr_app::pipeline::check_snapshot_utc(t)?;
    }
    let dir = project_dir(&a.project);
    let mut p = ExamProject::open(&dir, &a.operator)?;
    p.set_actor(&a.operator, "Instructor");
    let opts = GradeOptions {
        threads: a.threads,
        project_dir: None,
        snapshot_utc: a.snapshot_time.clone(),
        operator: a.operator.clone(),
        max_pages: None,
    };
    let r = p.add_scans(&a.inputs, &opts, &|_| {})?;
    println!(
        "{}: {} file(s), {} new page(s), {} already imported (CAP-011); booklets {} → {}; \
         {} review decision(s) carried over, {} conflict(s) (SYS-032) queued",
        r.batch,
        r.files,
        r.new_pages,
        r.duplicates,
        r.booklets_before,
        r.booklets_after,
        r.carried_over,
        r.conflicts
    );
    println!("{} open item(s)", p.queue(None).len());
    if !a.no_export {
        reexport(&mut p, &dir)?;
    }
    if let Some(why) = p.session.release_blocked() {
        println!("release blocked: {why}");
    }
    print_page_problems(&p);
    p.close()
}

fn cmd_release(
    project: &Path,
    reason: Option<&str>,
    note: Option<&str>,
    operator: &str,
) -> Result<(), AppError> {
    let dir = project_dir(project);
    let mut p = ExamProject::open(&dir, operator)?;
    p.set_actor(operator, "Instructor");
    p.release(reason_of(reason, note))?;
    let r = p
        .session
        .release
        .clone()
        .ok_or_else(|| AppError::Internal("release not recorded".into()))?;
    println!(
        "released: score run {}…, key version {}, results SHA-384 {}…",
        &r.score_run[..16.min(r.score_run.len())],
        r.key_version,
        &r.results_sha384[..16]
    );
    reexport(&mut p, &dir)?;
    let out = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    if out.join(omr_app::export::BLACKBOARD_FILE).exists() {
        println!(
            "Blackboard upload: {}",
            out.join(omr_app::export::BLACKBOARD_FILE).display()
        );
    } else {
        println!("no Blackboard upload: it needs a roster (rosterless exams are never pushed)");
    }
    let absent = p.absent_students().len();
    if absent > 0 {
        println!("{absent} roster student(s) handed in no sheet: listed as absent in results.csv");
    }
    p.close()
}

fn cmd_export(project: &Path, out: Option<&Path>, column: Option<&str>) -> Result<(), AppError> {
    let dir = project_dir(project);
    let mut p = ExamProject::open(&dir, "principal:operator")?;
    let out = match out {
        Some(o) => o.to_path_buf(),
        None => dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dir.clone()),
    };
    let s = p.export(&out)?;
    println!("exported {} file(s) to {}", s.files.len(), out.display());
    if let Some(c) = column {
        // Its own file: the manifest of `export` lists the new-column upload.
        let f = out.join(format!(
            "blackboard_grade_center-column-{}.csv",
            c.trim().trim_start_matches('|')
        ));
        p.export_blackboard(&f, Some(c))?;
        println!("Blackboard upload for column {c}: {}", f.display());
    } else if !s
        .files
        .iter()
        .any(|f| f == omr_app::export::BLACKBOARD_FILE)
    {
        println!(
            "no Blackboard upload: it needs released results (omr release --project {}), a \
             supplied key (not inferred), a roster and a score run",
            shell_arg(project)
        );
    }
    p.close()
}

fn cmd_key(cmd: KeyCmd) -> Result<(), AppError> {
    match cmd {
        KeyCmd::Replace {
            project,
            key,
            reason,
            note,
            operator,
            no_export,
        } => {
            let dir = project_dir(&project);
            let mut p = ExamProject::open(&dir, &operator)?;
            p.set_actor(&operator, "Instructor");
            let def = &p.session.definition;
            let (forms, points) =
                exam::read_key_file(&read_file(&key)?, &def.forms, def.questions, def.options)?;
            p.replace_key(forms, points, reason_of(reason.as_deref(), note.as_deref()))?;
            let k = p.session.key.as_ref().map_or(0, |k| k.version);
            println!("key version {k} in force; the exam was rescored");
            if !no_export {
                reexport(&mut p, &dir)?;
            }
            if let Some(why) = p.session.release_blocked() {
                println!("release blocked: {why}");
            }
            p.close()
        }
        KeyCmd::Set {
            project,
            form,
            question,
            answer,
            reason,
            note,
            operator,
            no_export,
        } => {
            let dir = project_dir(&project);
            let mut p = ExamProject::open(&dir, &operator)?;
            p.set_actor(&operator, "Instructor");
            let forms = &p.session.definition.forms;
            let f = match &form {
                None => 0,
                Some(f) => forms
                    .iter()
                    .position(|x| x.eq_ignore_ascii_case(f.trim()))
                    .ok_or_else(|| {
                        usage(format!(
                            "--form {f}: the exam's forms are {}",
                            forms.join(", ")
                        ))
                    })?,
            };
            p.set_key_with_reason(
                f,
                question,
                &answer,
                reason_of(reason.as_deref(), note.as_deref()),
            )?;
            let k = p.session.key.as_ref().map_or(0, |k| k.version);
            println!("key version {k} in force; the exam was rescored");
            if !no_export {
                reexport(&mut p, &dir)?;
            }
            p.close()
        }
    }
}

fn cmd_bench(b: BenchArgs) -> Result<(), AppError> {
    let def = {
        let mut d = definition(&b.exam, None)?;
        d.key = KeySource::Inferred;
        d.validate()?;
        d
    };
    let paths = omr_app::ingest::collect_paths(&b.corpus)?;
    let sources: Vec<omr_app::ingest::SourceFile> = paths
        .iter()
        .map(|p| {
            omr_app::ingest::SourceFile::read(p)
                .map_err(|e| AppError::Input(format!("{}: {e}", p.display())))
        })
        .collect::<Result<_, _>>()?;
    let mut runs = Vec::new();
    for &t in &b.threads {
        let pool = omr_app::pipeline::pool(t)?;
        let mut best: Option<(f64, f64, usize)> = None;
        for _ in 0..b.repeat.max(1) {
            // Core: decode + page stage + batch stage (recognition only).
            let t0 = Instant::now();
            let (pages, _, _) = omr_app::pipeline::recognize_only(&def, &sources, &pool)?;
            let core = t0.elapsed().as_secs_f64();
            // Full pipeline in memory (identity, inference, scoring, queue; no store).
            let t1 = Instant::now();
            let srcs = sources.iter().cloned().map(Ok).collect();
            let g = omr_app::pipeline::grade_sources(
                &def,
                srcs,
                &GradeOptions::default(),
                &pool,
                Instant::now(),
                &|_| {},
            )?;
            let full = t1.elapsed().as_secs_f64();
            drop(g);
            if best.is_none_or(|(c, _, _)| core < c) {
                best = Some((core, full, pages));
            }
        }
        let (core, full, pages) = best.expect("at least one run");
        println!(
            "{t} thread(s): {pages} pages · recognition {:.2} s = {:.1} pages/s · full pipeline (no store) {:.2} s = {:.1} pages/s",
            core,
            pages as f64 / core,
            full,
            pages as f64 / full
        );
        runs.push(json!({
            "threads": t, "pages": pages,
            "recognition_s": core, "recognition_pages_per_s": pages as f64 / core,
            "pipeline_s": full, "pipeline_pages_per_s": pages as f64 / full,
        }));
    }
    // Time to first result: the first `--first` sheets end to end (store + exports).
    let scratch = b
        .scratch
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("omr-bench"));
    std::fs::create_dir_all(&scratch)?;
    let first_dir = scratch.join(format!("ttfr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&first_dir);
    std::fs::create_dir_all(&first_dir)?;
    // The run spools copies of the originals: removed on every exit path, errors included.
    struct RemoveOnDrop(PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = RemoveOnDrop(first_dir.clone());
    let maxt = b.threads.iter().copied().max().unwrap_or(0);
    let pool = omr_app::pipeline::pool(maxt)?;
    let t0 = Instant::now();
    let opts = GradeOptions {
        threads: maxt,
        project_dir: Some(first_dir.join("project.omrproj")),
        snapshot_utc: Some("2026-10-16T00:00:00Z".into()),
        operator: "principal:bench".into(),
        max_pages: Some(b.first),
    };
    let srcs = sources.iter().cloned().map(Ok).collect();
    let g = omr_app::pipeline::grade_sources(&def, srcs, &opts, &pool, t0, &|_| {})?;
    let first_page_ms = g.session.timings.first_result_ms;
    let mut p = ExamProject::from_graded(g);
    p.export(&first_dir)?;
    let ttfr = t0.elapsed().as_secs_f64();
    let sheets = p.session.pages.len();
    p.close()?;
    println!(
        "time to first result: {sheets} sheets end to end (store + exports) in {ttfr:.2} s at {maxt} thread(s); first provisional page after {first_page_ms:.0} ms"
    );
    let report = json!({
        "corpus_files": sources.len(),
        "template": def.template.id(),
        "questions": def.questions,
        "runs": runs,
        "ttfr": {"sheets": sheets, "seconds": ttfr, "threads": maxt, "first_page_ms": first_page_ms},
    });
    if let Some(o) = &b.out {
        std::fs::write(
            o,
            serde_json::to_vec_pretty(&report).map_err(|e| AppError::Internal(e.to_string()))?,
        )?;
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), AppError> {
    match cli.cmd {
        Cmd::Templates { example_spec } => cmd_templates(example_spec),
        Cmd::Sheet {
            cmd:
                SheetCmd::Render {
                    spec,
                    out,
                    copies,
                    key_out,
                    new_key,
                },
        } => cmd_render(&spec, &out, copies, key_out.as_deref(), new_key),
        Cmd::Grade(g) => cmd_grade(g),
        Cmd::Add(a) => cmd_add(a),
        Cmd::Review { cmd } => cmd_review(cmd),
        Cmd::Key { cmd } => cmd_key(cmd),
        Cmd::Bench(b) => cmd_bench(b),
        Cmd::Release {
            project,
            reason,
            note,
            operator,
        } => cmd_release(&project, reason.as_deref(), note.as_deref(), &operator),
        Cmd::Export {
            project,
            out,
            blackboard_column,
        } => cmd_export(&project, out.as_deref(), blackboard_column.as_deref()),
    }
}

/// A path as one shell argument in a printed next command: quoted when it holds a space or a
/// character the shell would split on (`omr review list --project '/home/x/OMR Exams/cs211'`).
fn shell_arg(p: &Path) -> String {
    let s = p.display().to_string();
    if s.chars()
        .all(|c| c.is_alphanumeric() || "/._-+:,@".contains(c))
    {
        s
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // A panic is an internal error (exit 70), never Rust's default 101.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(cli)))
        .unwrap_or_else(|_| Err(AppError::Internal("unexpected panic (see above)".into())));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omr: {e}");
            ExitCode::from(e.exit_code().clamp(1, 255) as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shell_arg;
    use std::path::Path;

    /// A printed next command can be pasted as is, also for a folder with a space (README:
    /// `~/OMR Exams/…`).
    #[test]
    fn next_commands_quote_paths_the_shell_would_split() {
        assert_eq!(
            shell_arg(Path::new("out/cs211-midterm_1")),
            "out/cs211-midterm_1"
        );
        assert_eq!(
            shell_arg(Path::new("/home/u/OMR Exams/cs211")),
            "'/home/u/OMR Exams/cs211'"
        );
        assert_eq!(shell_arg(Path::new("it's")), "'it'\\''s'");
    }
}
