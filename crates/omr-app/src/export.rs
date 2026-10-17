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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSummary {
    /// File names written (relative to the export folder).
    pub files: Vec<String>,
    /// SHA-384 of the canonical export manifest.
    pub manifest_sha384: String,
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

/// Response code for exports: letter(s), `BLANK`, `MULTI:AC`, `?` pending, `UNREADABLE`.
pub fn response_text(r: &Response, labels: &[String]) -> String {
    let lab = |o: &u8| labels.get(usize::from(*o)).cloned().unwrap_or_default();
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

fn students(p: &ExamProject) -> BTreeMap<omr_model::StudentKey, StudentInfo> {
    p.session
        .definition
        .roster
        .iter()
        .flatten()
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
        .collect()
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
            "duplicate": count(PageStatus::Duplicate),
        },
        "booklets": s.booklets.len(),
        "questions_total": q_total,
        "questions_auto_accepted": q_auto,
        "review_rate": if q_total == 0 { 0.0 } else { (q_total - q_auto) as f64 / q_total as f64 },
        "open_items_by_code": by_code,
        "open_blocking": s.open_blocking(),
        "release_blocked": s.release_blocked(),
        "batch_exceptions": s.batch_exceptions.iter().map(|e| format!("{} {:?} {}", e.code, e.severity, e.sub_reason.clone().unwrap_or_default())).collect::<Vec<_>>(),
        "score_run": p.scoring.as_ref().map(|sc| match &sc.run {
            Ok(r) => json!({"run_id": r.run_id, "exceptions": r.exceptions.iter().filter(|e| e.severity >= Severity::Error).count()}),
            Err(e) => json!({"error": e}),
        }),
        "timings_ms": s.timings,
        "engine": s.manifest,
    })
}

/// Writes every export into `dir`.
pub fn write_all(p: &ExamProject, dir: &Path) -> Result<ExportSummary, AppError> {
    std::fs::create_dir_all(dir)?;
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
    let analytics = p.analytics();
    let outcomes = p.outcomes();
    if let Some(sc) = &p.scoring
        && let Ok(run) = &sc.run
    {
        let table = ResultsTable::from_run(
            m.clone(),
            run,
            &sc.prep.matrix,
            &GradeScale::saudi(),
            &RoundingPolicy::default(),
            &students(p),
        )
        .map_err(rep)?;
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
    let mut sm = summary_json(p);
    sm["banner"] = json!(inferred.then_some(INFERRED_BANNER));
    files.push((format!("summary{sfx}.json"), js(sm)?));
    let mut manifest = ExportManifest::new(&m, "grades", true);
    for (name, bytes) in &files {
        std::fs::write(dir.join(name), bytes)?;
        manifest.add(name, bytes);
    }
    let mbytes = manifest.to_canonical_json();
    std::fs::write(dir.join("manifest.json"), &mbytes)?;
    let mut names: Vec<String> = files.into_iter().map(|(n, _)| n).collect();
    names.push("manifest.json".into());
    Ok(ExportSummary {
        files: names,
        manifest_sha384: manifest.sha384_hex(),
    })
}
