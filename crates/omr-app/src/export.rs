//! Exports (§16.3, §17): results CSV/XLSX, item analysis, TP-154 tables, the HTML report, the
//! review queue as JSON, per-booklet responses, the effective key, a run summary and the
//! SHA-384 export manifest. An inferred key marks **every** output: an extra first header row
//! in each CSV, the title of every XLSX/HTML document and `-INFERRED` in every file name.

use crate::AppError;
use crate::app::ExamProject;
use crate::model::{PageStatus, ResponseStatus};
use crate::review::Lane;
use omr_model::{Response, Severity};
use omr_report::{
    ExportManifest, Headers, HtmlReport, ItemAnalysis, ReportMeta, ResultsTable, Sheet,
    StudentInfo, Tp154, Tp154Labels,
};
use omr_scoring::{GradeScale, RoundingPolicy};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// The warning printed on every output of an inferred-key run.
pub const INFERRED_BANNER: &str = "KEY INFERRED FROM THE SHEETS (modal confident response per form and question) - PROVISIONAL, NOT RELEASABLE (SCO-003 inferred_key)";

/// What was written.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSummary {
    /// File names written (relative to the export folder).
    pub files: Vec<String>,
    /// SHA-384 of the canonical export manifest.
    pub manifest_sha384: String,
    /// Item analysis + outcomes (ms).
    pub analysis_ms: f64,
    /// Everything (ms).
    pub total_ms: f64,
}

fn rep(e: omr_report::ReportError) -> AppError {
    AppError::Internal(format!("report: {e}"))
}

fn with_banner(mut s: Sheet, inferred: bool) -> Sheet {
    if inferred {
        s.headers.insert(0, vec![INFERRED_BANNER.to_string()]);
        s.title = format!("{INFERRED_BANNER} | {}", s.title);
    }
    s
}

fn csv_bytes(s: &Sheet) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::new();
    omr_report::write_csv(&mut out, s).map_err(rep)?;
    Ok(out)
}

/// Response code for exports: letter(s), `BLANK`, `MULTI:AC`, `?` pending, `UNREADABLE`. An
/// option beyond the exam's labels (a printed option the exam does not use) keeps its printed
/// letter, never an empty cell.
pub fn response_text(r: &Response, labels: &[String]) -> String {
    let lab = |o: &u8| {
        labels
            .get(usize::from(*o))
            .cloned()
            .unwrap_or_else(|| char::from(b'A' + (*o).min(25)).to_string())
    };
    match r {
        Response::Single(o) => lab(o),
        Response::Blank => "BLANK".into(),
        Response::Multi(v) | Response::Set(v) => {
            format!("MULTI:{}", v.iter().map(lab).collect::<String>())
        }
        Response::Unreadable { .. } => "UNREADABLE".into(),
    }
}

/// The report metadata of an exam.
pub fn meta(p: &ExamProject, title: &str) -> ReportMeta {
    let s = &p.session;
    let def = &s.definition;
    let inferred = s.key.as_ref().is_some_and(|k| k.inferred);
    let full = if inferred {
        format!("{title} - INFERRED KEY, NOT RELEASABLE")
    } else {
        title.to_string()
    };
    let mut m = ReportMeta::new(&full, &s.snapshot_utc);
    m.course_code = (!def.course_code.is_empty()).then(|| def.course_code.clone());
    m.course_name = (!def.course_title.is_empty()).then(|| def.course_title.clone());
    m.course_name_ar = (!def.course_title_ar.is_empty()).then(|| def.course_title_ar.clone());
    m.term = (!def.term.is_empty()).then(|| def.term.clone());
    m.engine_manifest = Some(s.manifest.config_sha384.clone());
    m.template_hash = Some(s.manifest.template_sha384.clone());
    if let Some(sc) = &p.scoring
        && let Ok(run) = &sc.run
    {
        m.score_run_id = Some(run.run_id.clone());
        m.key_version = Some(run.key_version);
        m.policy_version = Some(run.policy_version);
    }
    m.chain_head = p.store().and_then(|st| st.head()).map(|(_, h)| h);
    m.draft = inferred;
    m
}

/// Display data per student: the roster's; without a roster, the section bubbled on each
/// student's sheet (so a rosterless exam still splits by section).
fn students(p: &ExamProject) -> BTreeMap<omr_model::StudentKey, StudentInfo> {
    let s = &p.session;
    match &s.definition.roster {
        Some(rows) => rows
            .iter()
            .map(|r| {
                (
                    omr_model::StudentKey(omr_identity::normalize_id(&r.id)),
                    StudentInfo {
                        sis_id: Some(r.id.clone()),
                        name: (!r.name.is_empty()).then(|| r.name.clone()),
                        section: (!r.section.is_empty()).then(|| r.section.clone()),
                    },
                )
            })
            .collect(),
        None => s
            .booklets
            .iter()
            .filter(|b| !b.sheet_section.is_empty())
            .filter_map(|b| {
                let k = b.identity.student.clone()?;
                Some((
                    omr_model::StudentKey(k.clone()),
                    StudentInfo {
                        sis_id: Some(k),
                        name: None,
                        section: Some(b.sheet_section.clone()),
                    },
                ))
            })
            .collect(),
    }
}

/// The review queue as JSON (queue order per lane, then resolved items).
pub fn review_json(p: &ExamProject) -> Value {
    let s = &p.session;
    let labels = s.definition.option_labels();
    let item_json = |it: &crate::review::ReviewItem| {
        let b = it.booklet.map(|b| &s.booklets[b]);
        let page = it.page.map(|pi| &s.pages[pi]);
        json!({
            "id": it.id,
            "lane": it.lane.label(),
            "kind": format!("{:?}", it.kind),
            "code": it.code,
            "sub_reason": it.sub_reason,
            "severity": format!("{:?}", it.severity).to_uppercase(),
            "title": it.title,
            "detail": it.detail,
            "booklet": b.map(|b| b.booklet_id.clone()),
            "serial": b.map(|b| b.serial),
            "page_id": page.map(|p| p.page_id.clone()),
            "source": page.map(|p| s.sources[p.source].name.clone()),
            "source_page": page.map(|p| p.index + 1),
            "question": it.question.map(|q| q + 1),
            "suggestion": it.suggestion.as_ref().map(|d| d.text(&labels, &s.definition.forms)),
            "fills": it.fills.iter().map(|f| (f * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
            "group_key": format!("{:016x}", it.group_key),
            "crop": it.crop,
            "state": format!("{:?}", it.state),
            "resolution": it.resolution.as_ref().map(|d| d.text(&labels, &s.definition.forms)),
            "store_exceptions": it.store_exceptions,
        })
    };
    let mut lanes = serde_json::Map::new();
    for lane in Lane::ALL {
        lanes.insert(
            lane.label().into(),
            Value::Array(p.queue(Some(lane)).into_iter().map(item_json).collect()),
        );
    }
    let resolved: Vec<Value> = s
        .review
        .items
        .iter()
        .filter(|i| !i.is_open())
        .map(item_json)
        .collect();
    json!({
        "exam": s.definition.title,
        "key_inferred": s.key.as_ref().is_some_and(|k| k.inferred),
        "open": s.review.items.iter().filter(|i| i.is_open()).count(),
        "open_blocking": s.open_blocking(),
        "lanes": lanes,
        "resolved": resolved,
        "history": s.review.history,
    })
}

/// Per-booklet responses, forms, identity and scores.
pub fn booklets_json(p: &ExamProject) -> Value {
    let s = &p.session;
    let labels = s.definition.option_labels();
    let run = p.scoring.as_ref().and_then(|sc| sc.run.as_ref().ok());
    let grades = p.scoring.as_ref().map(|sc| sc.grades()).unwrap_or_default();
    let rows: Vec<Value> = s
        .booklets
        .iter()
        .enumerate()
        .map(|(bi, b)| {
            let row = p
                .scoring
                .as_ref()
                .and_then(|sc| sc.row_of.get(bi).copied().flatten());
            let score = row.and_then(|r| run.map(|run| &run.students[r]));
            let grade = row.and_then(|r| grades.get(r).cloned().flatten());
            json!({
                "booklet": b.booklet_id,
                "serial": b.serial,
                "pages": b.pages.iter().map(|&pi| {
                    let pg = &s.pages[pi];
                    json!({"page_id": pg.page_id, "source": s.sources[pg.source].name, "page": pg.index + 1})
                }).collect::<Vec<_>>(),
                "form": b.form.map(|f| s.definition.forms[f].clone()),
                "form_source": format!("{:?}", b.form_source),
                "declared": b.declared_text,
                "inference_margin_nats": b.inference.as_ref().map(|i| i.margin_milli as f64 / 1000.0),
                "student": b.identity.student,
                "section_on_sheet": b.sheet_section,
                "exam_type_on_sheet": b.exam_type,
                "id_read": b.identity.read,
                "identity_status": format!("{:?}", b.identity.status),
                "identity_codes": b.identity.exceptions.iter().map(|e| format!("{}{}", e.code, e.sub_reason.as_ref().map(|r| format!(":{r}")).unwrap_or_default())).collect::<Vec<_>>(),
                "responses": b.questions.iter().map(|q| response_text(&q.current, &labels)).collect::<Vec<_>>(),
                "auto": b.questions.iter().map(|q| q.status == ResponseStatus::Auto).collect::<Vec<_>>(),
                "status": b.questions.iter().map(|q| format!("{:?}", q.status)).collect::<Vec<_>>(),
                "codes": b.questions.iter().map(|q| q.recognized.exceptions.iter().map(|e| e.code.clone()).collect::<Vec<_>>()).collect::<Vec<_>>(),
                "score": score.map(|r| json!({
                    "status": format!("{:?}", r.status),
                    "raw": r.raw.to_string(),
                    "max": r.max.to_string(),
                    "percent": r.final_pct.map(|x| omr_scoring::decimal_string(&x, 2, omr_scoring::RoundingStrategy::MidpointAwayFromZero)),
                })),
                "grade": grade.map(|g| g.code().to_string()),
            })
        })
        .collect();
    json!({ "booklets": rows })
}

/// Run summary (counts, timings, gate).
pub fn summary_json(p: &ExamProject) -> Value {
    let s = &p.session;
    let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
    for it in s.review.items.iter().filter(|i| i.is_open()) {
        *by_code.entry(it.code.clone()).or_default() += 1;
    }
    let q_total: usize = s.booklets.iter().map(|b| b.questions.len()).sum();
    let q_auto: usize = s
        .booklets
        .iter()
        .flat_map(|b| &b.questions)
        .filter(|q| q.status == ResponseStatus::Auto)
        .count();
    let count = |st: PageStatus| s.pages.iter().filter(|p| p.status == st).count();
    let rescans = s
        .pages
        .iter()
        .filter(|p| p.refusal.as_ref().is_some_and(|e| e.code == "COL-003"))
        .count();
    json!({
        "exam": s.definition.title,
        "template": s.definition.template.id(),
        "questions": s.definition.questions,
        "forms": s.definition.forms,
        "key": match &s.key {
            None => "none",
            Some(k) if k.inferred => "INFERRED",
            Some(_) => "provided",
        },
        "pages": {
            "total": s.pages.len(),
            "read": count(PageStatus::Read),
            "refused": count(PageStatus::Refused),
            "unattributed": count(PageStatus::Unattributed),
            "duplicate": count(PageStatus::Duplicate) - rescans,
            "rescan_superseded": rescans,
            "blank": count(PageStatus::Blank),
        },
        "booklets": s.booklets.len(),
        // Roster students without a booklet (provisional while an identity item is open).
        "absent": p.absent_students().len(),
        "questions_total": q_total,
        "questions_auto_accepted": q_auto,
        "review_rate": if q_total == 0 { 0.0 } else { (q_total - q_auto) as f64 / q_total as f64 },
        "open_items_by_code": by_code,
        "open_blocking": s.open_blocking(),
        "release_blocked": s.release_blocked(),
        "released": s.release.as_ref().map(|r| json!({
            "at_utc": r.at_utc,
            "score_run": r.score_run,
            "key_version": r.key_version,
            "superseded_by": r.superseded,
        })),
        "batch_exceptions": s.batch_exceptions.iter().map(|e| format!("{} {:?} {}", e.code, e.severity, e.sub_reason.clone().unwrap_or_default())).collect::<Vec<_>>(),
        "score_run": p.scoring.as_ref().map(|sc| match &sc.run {
            Ok(r) => json!({"run_id": r.run_id, "exceptions": r.exceptions.iter().filter(|e| e.severity >= Severity::Error).count()}),
            Err(e) => json!({"error": e}),
        }),
        "timings_ms": s.timings,
        "engine": s.manifest,
    })
}

/// File name of the Blackboard Learn grade-center upload ([`blackboard_csv`]).
pub const BLACKBOARD_FILE: &str = "blackboard_grade_center.csv";

/// Removes Blackboard uploads written into `dir` by an earlier export (`BLACKBOARD_FILE` and the
/// CLI's `blackboard_grade_center-column-<ID>.csv`) once the results they carry are no longer
/// released.
fn remove_stale_uploads(dir: &Path) -> Result<(), AppError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let stale = name == BLACKBOARD_FILE
            || (name.starts_with("blackboard_grade_center-column-") && name.ends_with(".csv"));
        if stale && e.path().is_file() {
            std::fs::remove_file(e.path())?;
        }
    }
    Ok(())
}

/// A number without trailing zeros: `27`, `26.5`, `26.33` (2 decimals, half away from zero).
fn plain_number(x: &omr_scoring::Exact) -> String {
    let s = omr_scoring::decimal_string(x, 2, omr_scoring::RoundingStrategy::MidpointAwayFromZero);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// One quoted CSV field with the §16.3 formula guard (a leading `=`, `+`, `-`, `@`, TAB or CR
/// on a text field gets a `'` prefix).
fn quoted_text(s: &str) -> String {
    let guarded = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

/// Checks a Blackboard grade-column ID (the `|835932` suffix of a downloaded column header).
pub fn check_blackboard_column_id(id: &str) -> Result<(), AppError> {
    let t = id.trim().trim_start_matches('|');
    if t.is_empty() || t.len() > 32 || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(AppError::Input(format!(
            "Blackboard column ID `{id}`: copy the number after `|` in the column's header of a \
             Grade Center download (e.g. 835932), or leave it out to create a new column"
        )));
    }
    Ok(())
}

/// The Blackboard Learn grade-center upload ("Work offline", §16.3 LMS gradebook CSVs,
/// PAR-08; the REST connector of §16.1 is deferred), or `None` when the exam must not be
/// pushed: results not released (or changed since, §10.8), an inferred key (never
/// releasable), no roster (rosterless assessments are never pushed, §16.1) or no score run.
///
/// Format, from Blackboard's documentation: the upload is comma-delimited with one header row
/// and one record per line; students are matched by **Username** (the only student column
/// needed) and grade columns by the system's column ID, written as `|<id>` at the end of the
/// header; a header without an ID creates a new column. A downloaded grade column reads
/// `Quiz1 [Total Pts: 2.5 Score] |835932`.
/// - <https://help.anthology.com/blackboard/instructor/en/grading/work-offline-with-grade-data/upload-a-file-to-gradebook.html>
///   ("The unique identifier used for each student is the student's username. The unique
///   identifier for each column is a column ID number"; CSV or tab-delimited only)
/// - <https://help.blackboard.com/Learn/Instructor/Original/Grade/Grading_Tasks/Work_Offline_With_Grade_Data>
///   (Original course view: header row required, comma-delimited values in quotes)
/// - <https://blogs.ubc.ca/wtamagi/2014/11/06/using-datalink-sidekick-scanner-with-blackboard-connect/>
///   (a downloaded header: `Last Name, First Name, Username, Student ID, …, Quiz1 [Total
///   Pts: 2.5] |835932`)
///
/// Only what the upload needs is written (§16.3, data minimisation): `Username` = the roster
/// student ID (IAU Blackboard usernames are the university ID) and the raw score of the
/// students whose result is final (`Scored`, identity settled, no open blocking item on the
/// booklet, no open exam- or batch-scope item and no open unattributed page); everyone else is
/// left out, so Blackboard keeps
/// their current value. No names, sections or national IDs. CSV: UTF-8 with BOM, CRLF, every
/// field quoted; the Restricted label is carried by the export manifest (§16.3: CSV
/// "manifest only").
pub fn blackboard_csv(
    p: &ExamProject,
    column_id: Option<&str>,
) -> Result<Option<Vec<u8>>, AppError> {
    let s = &p.session;
    let def = &s.definition;
    if s.key.as_ref().is_none_or(|k| k.inferred) || def.roster.is_none() || !s.released() {
        return Ok(None);
    }
    let Some(sc) = &p.scoring else {
        return Ok(None);
    };
    let Ok(run) = &sc.run else {
        return Ok(None);
    };
    if let Some(id) = column_id {
        check_blackboard_column_id(id)?;
    }
    // Exam- and batch-scope blockers, and unattributed pages (no serial on these sheets:
    // they block every student of the batch, §13.1), hold everyone back.
    let blocked_all = s
        .review
        .items
        .iter()
        .any(|i| i.is_open() && i.severity >= Severity::Error && i.booklet.is_none());
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut max: Option<String> = None;
    if !blocked_all {
        for (bi, b) in s.booklets.iter().enumerate() {
            let Some(r) = sc.row_of.get(bi).copied().flatten() else {
                continue;
            };
            let res = &run.students[r];
            let open = s
                .review
                .items
                .iter()
                .any(|i| i.is_open() && i.severity >= Severity::Error && i.booklet == Some(bi));
            let Some(student) = b.identity.student.as_ref() else {
                continue;
            };
            if res.status != omr_scoring::AssessmentStatus::Scored || open {
                continue;
            }
            max.get_or_insert_with(|| plain_number(&res.max));
            rows.push((student.clone(), plain_number(&res.raw)));
        }
    }
    rows.sort();
    let name = [def.course_code.trim(), def.title.trim()]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let max = max.unwrap_or_else(|| {
        run.students
            .first()
            .map_or_else(|| "0".into(), |r| plain_number(&r.max))
    });
    let mut header = format!("{name} [Total Pts: {max} Score]");
    if let Some(id) = column_id {
        header.push_str(&format!(" |{}", id.trim().trim_start_matches('|')));
    }
    let mut out = String::from("\u{feff}");
    out.push_str(&format!(
        "{},{}\r\n",
        quoted_text("Username"),
        quoted_text(&header)
    ));
    for (user, score) in rows {
        out.push_str(&format!("{},\"{score}\"\r\n", quoted_text(&user)));
    }
    Ok(Some(out.into_bytes()))
}

/// `manifest.json` as written (§17.1): the canonical manifest (whose SHA-384 `ExportCreated`
/// records) with a `signature` member added: the device key's ECDSA P-384 signature over the
/// canonical manifest without that member, and the signed checkpoint taken just before the
/// export (its `ChainCheckpoint` event is the chain head the manifest cites). A verifier
/// removes `signature`, re-canonicalises and checks both. Without a project store (in-memory
/// runs) the manifest is written unsigned.
fn signed_manifest(p: &ExamProject, canonical: &[u8]) -> Result<Vec<u8>, AppError> {
    let Some(st) = p.store() else {
        return Ok(canonical.to_vec());
    };
    use omr_store::CheckpointSigner;
    let signer = crate::keys::device_signer()?;
    let sig = signer
        .sign(canonical)
        .map_err(|e| AppError::Store(format!("manifest signature: {e}")))?;
    let cp = st.latest_checkpoint()?.map(|c| {
        json!({
            "project": c.project, "seq": c.seq, "head_hash": c.head_hash,
            "hlc_ts": c.hlc_ts.to_string(), "alg": c.alg, "key_id": c.key_id,
            "public_key": hex::encode(&c.public_key), "signature": hex::encode(&c.signature),
        })
    });
    let mut v: Value = serde_json::from_slice(canonical)
        .map_err(|e| AppError::Internal(format!("manifest: {e}")))?;
    v["signature"] = json!({
        "alg": signer.algorithm(),
        "key_id": signer.key_id(),
        "public_key": hex::encode(signer.public_key()),
        "value": hex::encode(sig),
        "checkpoint": cp,
    });
    omr_store::jcs::value_to_jcs(&v)
        .map(String::into_bytes)
        .map_err(|e| AppError::Internal(format!("manifest: {e}")))
}

/// Review crops of `items` into `dir` (§9.10, §13.3): Restricted snippets of the sheets, so
/// they leave the project like any export: a checked destination (SYS-015), private files and
/// a signed `manifest.json` (label `restricted`, `pii: true`, purpose `review-crops`); the
/// caller records `ExportCreated`. Items without a crop (no stored page) are skipped.
pub fn write_crops(p: &ExamProject, dir: &Path, items: &[u64]) -> Result<ExportSummary, AppError> {
    let t0 = std::time::Instant::now();
    crate::keys::export_dir(dir)?;
    let def = &p.session.definition;
    let m = meta(p, &format!("{} review crops", def.title));
    let mut manifest = ExportManifest::new(&m, "review-crops", true);
    let mut names = Vec::new();
    for &id in items {
        let Ok(c) = p.crop(id) else {
            continue;
        };
        let png = crate::crops::png(&c).map_err(AppError::Internal)?;
        let name = format!("item-{id:05}.png");
        crate::keys::write_private(&dir.join(&name), &png)?;
        manifest.add(&name, &png);
        names.push(name);
    }
    let mbytes = manifest.to_canonical_json();
    crate::keys::write_private(&dir.join("manifest.json"), &signed_manifest(p, &mbytes)?)?;
    names.push("manifest.json".into());
    Ok(ExportSummary {
        files: names,
        manifest_sha384: manifest.sha384_hex(),
        analysis_ms: 0.0,
        total_ms: t0.elapsed().as_secs_f64() * 1e3,
    })
}

/// Masks student numbers in text shown outside the exports (a terminal, a log): every run of
/// seven or more digits keeps its last three (`•••••••123`), so scrollback and CI logs hold no
/// full student number (PDPL data minimisation); `--reveal` shows them and is recorded.
pub fn mask_numbers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 7 {
            out.extend(std::iter::repeat_n('•', run.len() - 3));
            out.push_str(&run[run.len() - 3..]);
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_digit() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// [`mask_numbers`] over every string of a JSON value, except the content-derived booklet and
/// page identifiers (hashes, not people).
pub fn mask_json(v: &mut Value) {
    match v {
        Value::String(s) => *s = mask_numbers(s),
        Value::Array(a) => a.iter_mut().for_each(mask_json),
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if !matches!(k.as_str(), "booklet" | "page_id" | "group_key") {
                    mask_json(x);
                }
            }
        }
        _ => {}
    }
}

/// Writes every export into `dir`.
pub fn write_all(p: &ExamProject, dir: &Path) -> Result<ExportSummary, AppError> {
    crate::keys::export_dir(dir)?;
    let s = &p.session;
    let def = &s.definition;
    let inferred = s.key.as_ref().is_some_and(|k| k.inferred);
    let sfx = if inferred { "-INFERRED" } else { "" };
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let title = if def.course_code.is_empty() {
        def.title.clone()
    } else {
        format!("{} {}", def.course_code, def.title)
    };
    let m = meta(p, &title);
    let t0 = std::time::Instant::now();
    let analytics = p.analytics();
    let outcomes = p.outcomes();
    let analysis_ms = t0.elapsed().as_secs_f64() * 1e3;
    if let Some(sc) = &p.scoring
        && let Ok(run) = &sc.run
    {
        let mut table = ResultsTable::from_run(
            m.clone(),
            run,
            &sc.prep.matrix,
            &GradeScale::saudi(),
            &RoundingPolicy::default(),
            &students(p),
        )
        .map_err(rep)?;
        // Roster students who handed in no sheet: status `absent` (not 0, §10.5), no booklet,
        // no score; never in the Blackboard upload.
        for r in p.absent_students() {
            table.rows.push(omr_report::ResultRow {
                student_id: Some(r.id.clone()),
                name: (!r.name.is_empty()).then(|| r.name.clone()),
                section: (!r.section.is_empty()).then(|| r.section.clone()),
                booklet: 0,
                form: String::new(),
                status: "absent".into(),
                raw: None,
                max: None,
                percent: None,
                letter: None,
                letter_ar: None,
                gpa5: None,
                gpa4: None,
                passing: None,
                responses: vec![String::new(); table.items.len()],
            });
        }
        files.push((
            format!("results{sfx}.csv"),
            csv_bytes(&with_banner(table.sheet(), inferred))?,
        ));
        let mut x = Vec::new();
        omr_report::write_xlsx(
            &mut x,
            &m,
            &[with_banner(table.sheet(), inferred), table.about_sheet()],
        )
        .map_err(rep)?;
        files.push((format!("results{sfx}.xlsx"), x));
        if let Some(a) = &analytics {
            let ia = ItemAnalysis {
                meta: &m,
                report: a,
                show_key: false,
            };
            files.push((
                format!("item_analysis{sfx}.csv"),
                csv_bytes(&with_banner(ia.items_sheet(), inferred))?,
            ));
            files.push((
                format!("test_stats{sfx}.csv"),
                csv_bytes(&with_banner(ia.tests_sheet(), inferred))?,
            ));
            let mut x = Vec::new();
            omr_report::write_xlsx(
                &mut x,
                &m,
                &[
                    with_banner(ia.items_sheet(), inferred),
                    with_banner(ia.tests_sheet(), inferred),
                    with_banner(ia.flags_sheet(), inferred),
                ],
            )
            .map_err(rep)?;
            files.push((format!("item_analysis{sfx}.xlsx"), x));
        }
        let labels = Tp154Labels::default();
        let tp = Tp154 {
            meta: &m,
            labels: &labels,
            headers: Headers::Both,
        };
        let a1 = outcomes
            .as_ref()
            .map(|o| o.a1.clone())
            .unwrap_or_else(|| crate::outcomes::a1_only(sc));
        files.push((
            format!("tp154_a1{sfx}.csv"),
            csv_bytes(&with_banner(tp.a1_sheet(&a1), inferred))?,
        ));
        let mut sheets = vec![with_banner(tp.a1_sheet(&a1), inferred)];
        if let Some(o) = &outcomes {
            let mut b1 = o.b1.clone();
            b1.draft_watermark |= inferred;
            files.push((
                format!("tp154_b1{sfx}.csv"),
                csv_bytes(&with_banner(tp.b1_sheet(&b1), inferred))?,
            ));
            sheets.push(with_banner(tp.b1_sheet(&b1), inferred));
        }
        let mut x = Vec::new();
        omr_report::write_xlsx(&mut x, &m, &sheets).map_err(rep)?;
        files.push((format!("tp154{sfx}.xlsx"), x));
        let html = HtmlReport {
            results: &table,
            scale: &GradeScale::saudi(),
            test: analytics.as_ref().and_then(|a| a.tests.first()),
            items: analytics.as_ref(),
            show_key: false,
            clos: outcomes.as_ref().map(|o| (&o.result, &o.clos)),
            open_exceptions: Some(s.open_blocking()),
        }
        .render();
        let html = if inferred {
            html.replacen(
                "<body",
                &format!("<!-- {INFERRED_BANNER} --><body"),
                1,
            )
            .replacen(
                "</h1>",
                &format!("</h1><p role=\"alert\" style=\"border:2px solid #b42318;padding:8px;font-weight:700\">{INFERRED_BANNER}</p>"),
                1,
            )
        } else {
            html
        };
        files.push((format!("report{sfx}.html"), html.into_bytes()));
    }
    // Effective key.
    if let Some(k) = &s.key {
        let mut out = String::new();
        if inferred {
            out.push_str(INFERRED_BANNER);
            out.push_str("\r\n");
        }
        out.push_str("question");
        let forms: Vec<&String> = if k.shared_items {
            def.forms.iter().take(1).collect()
        } else {
            def.forms.iter().collect()
        };
        for f in &forms {
            out.push(',');
            out.push_str(f);
        }
        out.push_str(",points\r\n");
        for q in 0..usize::from(def.questions) {
            out.push_str(&(q + 1).to_string());
            for (fi, _) in forms.iter().enumerate() {
                out.push(',');
                out.push_str(&k.forms[fi][q].text());
            }
            out.push(',');
            out.push_str(k.points.get(q).map_or("1", String::as_str));
            out.push_str("\r\n");
        }
        files.push((format!("key{sfx}.csv"), out.into_bytes()));
    }
    let js =
        |v: Value| serde_json::to_vec_pretty(&v).map_err(|e| AppError::Internal(e.to_string()));
    let mut rq = review_json(p);
    rq["banner"] = json!(inferred.then_some(INFERRED_BANNER));
    files.push((format!("review_queue{sfx}.json"), js(rq)?));
    let mut bk = booklets_json(p);
    bk["banner"] = json!(inferred.then_some(INFERRED_BANNER));
    files.push((format!("booklets{sfx}.json"), js(bk)?));
    match blackboard_csv(p, None)? {
        Some(bb) => files.push((BLACKBOARD_FILE.into(), bb)),
        // No upload now (not released, or a later change superseded the release): an upload
        // left from before would carry grades that are no longer the released ones.
        None => remove_stale_uploads(dir)?,
    }
    let mut sm = summary_json(p);
    sm["banner"] = json!(inferred.then_some(INFERRED_BANNER));
    files.push((format!("summary{sfx}.json"), js(sm)?));
    let mut manifest = ExportManifest::new(&m, "grades", true);
    for (name, bytes) in &files {
        crate::keys::write_private(&dir.join(name), bytes)?;
        manifest.add(name, bytes);
    }
    let mbytes = manifest.to_canonical_json();
    crate::keys::write_private(&dir.join("manifest.json"), &signed_manifest(p, &mbytes)?)?;
    let mut names: Vec<String> = files.into_iter().map(|(n, _)| n).collect();
    names.push("manifest.json".into());
    Ok(ExportSummary {
        files: names,
        manifest_sha384: manifest.sha384_hex(),
        analysis_ms,
        total_ms: t0.elapsed().as_secs_f64() * 1e3,
    })
}
