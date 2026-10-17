//! `omr`: headless command-line front end (spec §4.3, §16.4).
//!
//! ```text
//! omr templates
//! omr sheet render --spec spec.json --out sheets.pdf [--copies N]
//! omr grade <inputs…> --template iau-ccsit-v1 | --spec spec.json --questions N
//!           [--key key.csv | --infer-key] [--roster roster.csv|.xlsx] [--clo clo.csv] --out DIR
//! omr review list --project DIR [--lane marks] [--json]
//! omr review resolve --project DIR --item ID (--decision B | --defer | --escalate | --rescan | --undo)
//! omr bench <corpus-dir> --template … --questions N [--threads 1,2,4] [--first 30]
//! ```
//!
//! Exit codes: 0 success; 2 usage or exam definition; 3 input; 4 project store; 5 scoring;
//! 6 review; 7 not found; 8 I/O; 70 internal.
#![forbid(unsafe_code)]

use clap::{Args, Parser, Subcommand};
use omr_app::exam::{self, IdSchemeChoice};
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
    /// Review queue of a graded exam.
    Review {
        #[command(subcommand)]
        cmd: ReviewCmd,
    },
    /// Throughput (A2) and time to first result (A3) on a corpus folder.
    Bench(BenchArgs),
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
    /// Student-ID scheme: iau | pnu | ksu | digits:N.
    #[arg(long, default_value = "iau")]
    id_scheme: String,
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
        /// Write each item's crop as PNG into this folder.
        #[arg(long)]
        crops: Option<PathBuf>,
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
        #[arg(long)]
        group: bool,
        /// Defer to the end of the queue.
        #[arg(long)]
        defer: bool,
        /// Escalate to the instructor.
        #[arg(long)]
        escalate: bool,
        /// Add to the rescan pull list.
        #[arg(long)]
        rescan: bool,
        /// Undo the latest review event.
        #[arg(long)]
        undo: bool,
        /// Reviewer principal.
        #[arg(long, default_value = "principal:reviewer")]
        reviewer: String,
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
            let opts = match spec.options {
                omr_forms::OptionCounts::Uniform(n) => n,
                _ => 5,
            };
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
    d.id_scheme = match a.id_scheme.as_str() {
        "iau" => IdSchemeChoice::Iau,
        "pnu" => IdSchemeChoice::Pnu,
        "ksu" => IdSchemeChoice::Ksu,
        s => match s.strip_prefix("digits:").and_then(|n| n.parse().ok()) {
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
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let first = text
        .lines()
        .next()?
        .trim_start_matches('\u{feff}')
        .to_lowercase();
    let cols: Vec<String> = first.split(',').map(|c| c.trim().to_string()).collect();
    match cols.first().map(String::as_str) {
        Some("question" | "q" | "item") => {
            let f: Vec<String> = cols[1..]
                .iter()
                .filter(|c| !matches!(c.as_str(), "points" | "marks" | "key" | "answer"))
                .map(|c| c.to_uppercase())
                .collect();
            (!f.is_empty()).then_some(f)
        }
        Some("form" | "version") => Some(
            text.lines()
                .skip(1)
                .filter_map(|l| l.split(',').next())
                .map(|f| f.trim().to_uppercase())
                .filter(|f| !f.is_empty())
                .collect(),
        ),
        _ => None,
    }
}

fn cmd_templates(example: bool) -> Result<(), AppError> {
    if example {
        let spec = TemplateSpec::standard(
            omr_model::ExamId(
                omr_app::exam::ExamDefinition::new(
                    "Example",
                    TemplateChoice::Legacy("iau-ccsit-v1".into()),
                    1,
                    5,
                    vec!["A".into()],
                    CancelMode::EraseOnly,
                )
                .exam_id
                .0,
            ),
            40,
            4,
            CancelMode::EraseOnly,
        );
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

fn cmd_render(spec: &Path, out: &Path, copies: u32) -> Result<(), AppError> {
    let (spec, key_hex) = read_spec(spec)?;
    let key_hex = match key_hex {
        Some(k) => k,
        None => {
            let mut k = [0u8; 32];
            rand::fill(&mut k);
            let hexk = hex::encode(k);
            let kp = out.with_extension("exam-key.json");
            std::fs::write(
                &kp,
                serde_json::to_vec_pretty(&json!({"template": spec, "exam_key_hex": hexk}))
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )?;
            eprintln!(
                "generated a new page-code key; grading needs `--spec {}` (keep it secret)",
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
        let (forms, points) = exam::read_key_csv(&read_file(k)?, &d.forms, d.questions)?;
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
        d.roster = Some(if ext == "xlsx" || ext == "xlsm" {
            exam::read_roster_xlsx(&bytes)?
        } else {
            exam::read_roster_csv(&bytes)?
        });
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
    std::fs::create_dir_all(&g.out)?;
    let proj = g.out.join("project.omrproj");
    if proj.exists() {
        return Err(AppError::Input(format!(
            "{} already holds a graded exam; use a new --out folder",
            g.out.display()
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
    if let Some(why) = s.release_blocked() {
        println!("release blocked: {why}");
    }
    println!("wrote {} file(s) to {}", sum.files.len(), g.out.display());
    p.close()
}

fn cmd_review(cmd: ReviewCmd) -> Result<(), AppError> {
    match cmd {
        ReviewCmd::List {
            project,
            lane,
            json: as_json,
            crops,
        } => {
            let p = ExamProject::open(&project_dir(&project), "principal:viewer")?;
            let lane = match lane.as_deref() {
                None => None,
                Some(l) => Some(Lane::parse(l).ok_or_else(|| {
                    usage(format!(
                        "--lane {l}: use marks, identity, capture or scoring"
                    ))
                })?),
            };
            let items = p.queue(lane);
            if let Some(dir) = &crops {
                std::fs::create_dir_all(dir)?;
                for it in &items {
                    if let Ok(c) = p.crop(it.id) {
                        let png = omr_app::crops::png(&c).map_err(AppError::Internal)?;
                        std::fs::write(dir.join(format!("item-{:05}.png", it.id)), png)?;
                    }
                }
            }
            if as_json {
                let v = omr_app::export::review_json(&p);
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
                        it.suggestion.as_ref().map_or_else(String::new, |d| format!(
                            "suggest {}",
                            d.text(&labels, &p.session.definition.forms)
                        ))
                    );
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
            reviewer,
            no_export,
        } => {
            let dir = project_dir(&project);
            let mut p = ExamProject::open(&dir, &reviewer)?;
            p.set_actor(&reviewer, "Reviewer");
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
                let out = p.apply(id, action)?;
                println!(
                    "recorded; affected {:?}; next {}",
                    out.affected,
                    out.next.map_or("none".into(), |n| n.to_string())
                );
            }
            if !no_export {
                let out = dir
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| dir.clone());
                let s = p.export(&out)?;
                println!("re-exported {} file(s) to {}", s.files.len(), out.display());
            }
            println!("{} open item(s) remain", p.queue(None).len());
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
    let _ = std::fs::remove_dir_all(&first_dir);
    Ok(())
}

fn run(cli: Cli) -> Result<(), AppError> {
    match cli.cmd {
        Cmd::Templates { example_spec } => cmd_templates(example_spec),
        Cmd::Sheet {
            cmd: SheetCmd::Render { spec, out, copies },
        } => cmd_render(&spec, &out, copies),
        Cmd::Grade(g) => cmd_grade(g),
        Cmd::Review { cmd } => cmd_review(cmd),
        Cmd::Bench(b) => cmd_bench(b),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omr: {e}");
            ExitCode::from(e.exit_code().clamp(1, 255) as u8)
        }
    }
}
