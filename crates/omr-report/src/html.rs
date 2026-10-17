//! Self-contained single-file HTML results report (spec §17.1–§17.2; docs/design/DESIGN.md
//! §3, §9.5).
//!
//! One file, no external assets: inline CSS with the design tokens (light and dark through
//! `prefers-color-scheme`), inline SVG charts, no scripts, fonts named but never fetched.
//! The classification label (default "Restricted / مقيد") heads the page and closes it; every
//! user or Arabic text node renders with `dir="auto"`; the document date is the snapshot time.
//! Charts follow the design rules: one series in `series-1` with no legend, F hatched in
//! `error` with the word "Fail", values in ink, a table view beside every chart, the target as
//! an ink line.

use crate::error::ReportError;
use crate::items::{band_name, flag_label, flag_order};
use crate::meta::Language;
use crate::results::ResultsTable;
use crate::text::{exact_str, float_str, html_escape, option_code};
use omr_analytics::{AnalyticsReport, Discrimination, TestStats};
use omr_outcomes::{AttainmentResult, CloSet};
use omr_scoring::{Exact, GradeScale};
use std::fmt::Write as _;
use std::io::Write;

/// Inputs of the HTML report.
#[derive(Clone, Copy, Debug)]
pub struct HtmlReport<'a> {
    /// Results (with the report metadata).
    pub results: &'a ResultsTable,
    /// Grade scale (bar order and Arabic letters).
    pub scale: &'a GradeScale,
    /// Test statistics (α, SEM) of the main item-set class.
    pub test: Option<&'a TestStats>,
    /// Item analysis.
    pub items: Option<&'a AnalyticsReport>,
    /// Show the keyed option in the item table.
    pub show_key: bool,
    /// CLO attainment with the CLO set (titles).
    pub clos: Option<(&'a AttainmentResult, &'a CloSet)>,
    /// Open exceptions in scope.
    pub open_exceptions: Option<usize>,
}

/// One bar of the grade distribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GradeBar {
    /// Latin letter.
    pub letter: String,
    /// Arabic letter.
    pub arabic: String,
    /// Count.
    pub count: usize,
}

/// Grade distribution of the graded rows, top letter first.
pub fn grade_distribution(results: &ResultsTable, scale: &GradeScale) -> Vec<GradeBar> {
    scale
        .bands
        .iter()
        .rev()
        .map(|b| GradeBar {
            letter: b.letter.clone(),
            arabic: b.arabic.clone(),
            count: results
                .rows
                .iter()
                .filter(|r| r.letter.as_deref() == Some(b.letter.as_str()))
                .count(),
        })
        .collect()
}

struct Text {
    lang: &'static str,
    dir: &'static str,
    students: &'static str,
    mean: &'static str,
    median: &'static str,
    sd: &'static str,
    pass_rate: &'static str,
    alpha: &'static str,
    sem: &'static str,
    open_exceptions: &'static str,
    distribution: &'static str,
    clo: &'static str,
    items: &'static str,
    results: &'static str,
    table_view: &'static str,
    count: &'static str,
    percent: &'static str,
    letter: &'static str,
    fail: &'static str,
    attained: &'static str,
    not_attained: &'static str,
    target: &'static str,
}

const EN: Text = Text {
    lang: "en",
    dir: "ltr",
    students: "Students",
    mean: "Mean",
    median: "Median",
    sd: "SD",
    pass_rate: "Pass rate",
    alpha: "Cronbach α",
    sem: "SEM",
    open_exceptions: "Open exceptions",
    distribution: "Grade distribution",
    clo: "CLO attainment",
    items: "Item analysis",
    results: "Student results",
    table_view: "Table view",
    count: "Count",
    percent: "Percent",
    letter: "Grade",
    fail: "Fail",
    attained: "Attained",
    not_attained: "Not attained",
    target: "Target",
};

const AR: Text = Text {
    lang: "ar",
    dir: "rtl",
    students: "الطلاب",
    mean: "المتوسط",
    median: "الوسيط",
    sd: "الانحراف المعياري",
    pass_rate: "نسبة النجاح",
    alpha: "معامل ألفا",
    sem: "الخطأ المعياري للقياس",
    open_exceptions: "الاستثناءات المفتوحة",
    distribution: "توزيع التقديرات",
    clo: "تحقق نواتج تعلم المقرر",
    items: "تحليل الأسئلة",
    results: "نتائج الطلاب",
    table_view: "عرض جدولي",
    count: "العدد",
    percent: "النسبة",
    letter: "التقدير",
    fail: "راسب",
    attained: "متحقق",
    not_attained: "غير متحقق",
    target: "المستهدف",
};

const CSS: &str = r#":root{color-scheme:light dark;
--canvas:#F5F0E6;--surface:#FFFDF8;--raised:#FFFFFF;--sunken:#F0EADF;--border:#E3DACB;--border-strong:#8C8270;--track:#E8E1D3;
--ink:#1C2430;--ink-2:#55606E;--ink-3:#5D6672;--primary:#0E5E57;--primary-soft:#DCEBE6;--accent-ink:#7A5815;--accent-soft:#F6EBD3;
--success:#2A7148;--success-soft:#E2EFE5;--warning:#9A5B00;--warning-soft:#FAEBD3;--error:#B3261E;--error-soft:#F7E0DC;--info:#3D5A80;--info-soft:#E2E8F0;
--series-1:#08856A;--series-2:#B7810F;--chart-grid:#ECE5D8;--chart-axis:#C9BFAE;
--sans:"IBM Plex Sans",system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;--arabic:"IBM Plex Sans Arabic","Noto Naskh Arabic","Segoe UI",Tahoma,sans-serif;--mono:"IBM Plex Mono",ui-monospace,"SFMono-Regular",Menlo,Consolas,monospace}
@media (prefers-color-scheme:dark){:root{
--canvas:#0F1716;--surface:#16211F;--raised:#1D2A28;--sunken:#121B1A;--border:#2A3936;--border-strong:#6B7C78;--track:#2A3936;
--ink:#ECE6DA;--ink-2:#B7BFBB;--ink-3:#8F9C98;--primary:#5CC2B1;--primary-soft:#173A35;--accent-ink:#E2BD6E;--accent-soft:#3A2F17;
--success:#6BCB8F;--success-soft:#18352A;--warning:#E7A846;--warning-soft:#3A2C12;--error:#F08A7E;--error-soft:#3E1F1C;--info:#9DB4DA;--info-soft:#1E2A3D;
--series-1:#16A384;--series-2:#BD871C;--chart-grid:#24312F;--chart-axis:#3A4A47}}
*{box-sizing:border-box}
body{margin:0;background:var(--canvas);color:var(--ink);font:13px/1.45 var(--sans)}
:lang(ar),.ar{font-family:var(--arabic)}
main{max-width:1180px;margin:0 auto;padding:16px}
.classification{display:flex;align-items:center;gap:8px;padding:8px 16px;background:var(--warning-soft);color:var(--warning);font-weight:600;border-bottom:1px solid var(--border)}
.classification svg{flex:none}
.watermark{margin:12px 0;padding:8px 12px;border:1px dashed var(--error);color:var(--error);background:var(--error-soft);font-weight:600}
header.report{padding:16px 0 8px}
h1{font-size:20px;font-weight:600;margin:0 0 4px}
h2{font-size:16px;font-weight:600;margin:0 0 12px}
.sub{color:var(--ink-2)}
.mono,td.num,.id{font-family:var(--mono);font-variant-numeric:tabular-nums}
.stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:12px;margin:12px 0}
.tile{background:var(--surface);border:1px solid var(--border);border-radius:8px;padding:12px}
.tile .k{font-size:11px;font-weight:600;letter-spacing:.6px;text-transform:uppercase;color:var(--ink-2)}
.tile .v{font-size:24px;font-weight:600}
.tile .s{font-size:12px;color:var(--ink-3)}
section.card{background:var(--surface);border:1px solid var(--border);border-radius:8px;padding:16px;margin:16px 0}
.grid2{display:grid;grid-template-columns:minmax(0,430px) minmax(0,1fr);gap:16px}
@media (max-width:860px){.grid2{grid-template-columns:1fr}}
table{border-collapse:collapse;width:100%}
th,td{padding:4px 8px;border-bottom:1px solid var(--border);text-align:start;vertical-align:top}
th{background:var(--sunken);font-size:12px;font-weight:600;color:var(--ink-2)}
td.num,th.num{text-align:end}
.scroll{overflow-x:auto}
details summary{cursor:pointer;color:var(--primary);margin-top:8px}
svg text{fill:var(--ink);font-family:var(--sans)}
svg .count{font-family:var(--mono);font-size:11px}
svg .ar{font-family:var(--arabic);font-size:12px}
svg .axis{stroke:var(--chart-axis)}
svg .grid{stroke:var(--chart-grid)}
svg .bar{fill:var(--series-1)}
svg .bar-fail{fill:url(#hatch-fail);stroke:var(--error);stroke-width:1}
svg .hatch-bg{fill:var(--error-soft)}
svg .hatch-line{stroke:var(--error);stroke-width:2}
svg .fail-word{fill:var(--error);font-size:10px;font-weight:600}
.pill{display:inline-block;padding:1px 8px;border-radius:999px;font-size:12px;font-weight:500;white-space:nowrap}
.pill.ok{background:var(--success-soft);color:var(--success)}
.pill.bad{background:var(--error-soft);color:var(--error)}
.pill.warn{background:var(--warning-soft);color:var(--warning)}
.pill.info{background:var(--info-soft);color:var(--info)}
.neg{color:var(--error);font-weight:600}
.clo{display:grid;grid-template-columns:minmax(0,1.3fr) minmax(0,2fr) auto;gap:12px;align-items:center;padding:8px 0;border-bottom:1px solid var(--border)}
.meter{position:relative;height:10px;background:var(--track);border-radius:5px;margin:3px 0}
.meter>.fill{position:absolute;inset-block:0;inset-inline-start:0;border-radius:5px}
.meter>.fill.mu{background:var(--series-1)}
.meter>.fill.pi{background:var(--series-2)}
.meter>.target{position:absolute;top:-3px;bottom:-3px;width:2px;background:var(--ink)}
.mlabel{display:flex;justify-content:space-between;font-size:12px;color:var(--ink-2)}
.legend{display:flex;gap:16px;font-size:12px;color:var(--ink-2)}
.sw{display:inline-block;width:10px;height:10px;border-radius:2px;margin-inline-end:4px;vertical-align:middle}
footer.prov{margin:24px 0 8px;padding-top:8px;border-top:1px solid var(--border);color:var(--ink-2);font-size:12px}
footer.prov dl{display:grid;grid-template-columns:max-content 1fr;gap:2px 12px;margin:8px 0}
footer.prov dt{font-weight:600}
@media print{body{background:#fff}section.card,.tile{border-color:#999}.classification{-webkit-print-color-adjust:exact;print-color-adjust:exact}}
"#;

const LOCK: &str = r#"<svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true"><rect x="3" y="7" width="10" height="8" rx="1.5" fill="currentColor"/><path d="M5 7V5a3 3 0 0 1 6 0v2" fill="none" stroke="currentColor" stroke-width="1.6"/></svg>"#;

fn to_f(x: &Exact) -> f64 {
    let (n, d) = (*x.numer(), *x.denom());
    n as f64 / d as f64
}

fn esc(s: &str) -> String {
    html_escape(s)
}

/// Median of sorted exact values (mean of the two middle ones for even N).
fn median(v: &[Exact]) -> Option<Exact> {
    let n = v.len();
    if n == 0 {
        return None;
    }
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / Exact::from_integer(2)
    })
}

impl HtmlReport<'_> {
    fn t(&self) -> &'static Text {
        match self.results.meta.language {
            Language::En => &EN,
            Language::Ar => &AR,
        }
    }

    /// Renders the page.
    pub fn render(&self) -> String {
        let m = &self.results.meta;
        let t = self.t();
        let mut h = String::with_capacity(32 * 1024);
        let label = esc(&m.classification.label());
        let _ = write!(
            h,
            "<!DOCTYPE html>\n<html lang=\"{}\" dir=\"{}\">\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
             <meta name=\"classification\" content=\"{label}\">\n\
             <meta name=\"generator\" content=\"omr-report\">\n<title>{}</title>\n<style>\n{CSS}</style>\n</head>\n<body>\n",
            t.lang,
            t.dir,
            esc(&m.title),
        );
        let _ = write!(
            h,
            "<div class=\"classification\" role=\"note\" aria-label=\"Classification\">{LOCK}<span dir=\"auto\">{label}</span></div>\n<main>\n"
        );
        if m.draft {
            h.push_str("<div class=\"watermark\" role=\"note\">DRAFT MAPPINGS — not for submission</div>\n");
        }
        if let Some(s) = &m.superseded_by {
            let _ = writeln!(
                h,
                "<div class=\"watermark\" role=\"note\" dir=\"auto\">SUPERSEDED by {}</div>",
                esc(s)
            );
        }
        self.header(&mut h);
        self.stats(&mut h);
        h.push_str("<div class=\"grid2\">\n");
        self.distribution(&mut h);
        self.clos(&mut h);
        h.push_str("</div>\n");
        self.items(&mut h);
        self.students(&mut h);
        self.footer(&mut h);
        h.push_str("</main>\n</body>\n</html>\n");
        h
    }

    /// Writes the page to `w`.
    pub fn write<W: Write>(&self, w: &mut W) -> Result<(), ReportError> {
        w.write_all(self.render().as_bytes())?;
        Ok(())
    }

    fn header(&self, h: &mut String) {
        let m = &self.results.meta;
        let _ = write!(
            h,
            "<header class=\"report\">\n<h1 dir=\"auto\">{}</h1>\n",
            esc(&m.title)
        );
        if let Some(a) = &m.title_ar {
            let _ = writeln!(
                h,
                "<div class=\"ar\" lang=\"ar\" dir=\"auto\">{}</div>",
                esc(a)
            );
        }
        let mut parts = Vec::new();
        if let Some(i) = &m.institution {
            parts.push(format!("<span dir=\"auto\">{}</span>", esc(i)));
        }
        let course = [m.course_code.as_deref(), m.course_name.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        if !course.is_empty() {
            parts.push(format!("<span dir=\"auto\">{}</span>", esc(&course)));
        }
        if let Some(a) = &m.course_name_ar {
            parts.push(format!("<span lang=\"ar\" dir=\"auto\">{}</span>", esc(a)));
        }
        if let Some(x) = &m.term {
            parts.push(format!("<span dir=\"auto\">{}</span>", esc(x)));
        }
        if let Some(x) = &m.scope {
            parts.push(format!("<span dir=\"auto\">{}</span>", esc(x)));
        }
        parts.push(format!(
            "<time class=\"mono\" datetime=\"{0}\">{0}</time>",
            esc(&m.snapshot_utc)
        ));
        let _ = write!(
            h,
            "<div class=\"sub\">{}</div>\n</header>\n",
            parts.join(" · ")
        );
    }

    fn tile(h: &mut String, k: &str, v: &str, s: &str) {
        let _ = writeln!(
            h,
            "<div class=\"tile\"><div class=\"k\">{}</div><div class=\"v mono\">{}</div><div class=\"s\">{}</div></div>",
            esc(k),
            esc(v),
            esc(s)
        );
    }

    fn stats(&self, h: &mut String) {
        let t = self.t();
        let rows = &self.results.rows;
        let d = self.results.decimals;
        let mut pcts: Vec<Exact> = rows.iter().filter_map(|r| r.percent).collect();
        pcts.sort();
        let graded = pcts.len();
        h.push_str("<div class=\"stats\">\n");
        Self::tile(
            h,
            t.students,
            &graded.to_string(),
            &format!("of {} listed", rows.len()),
        );
        if graded > 0 {
            let n = Exact::from_integer(graded as i128);
            let mean: Exact = pcts.iter().copied().sum::<Exact>() / n;
            let mf = to_f(&mean);
            let var = pcts.iter().map(|x| (to_f(x) - mf).powi(2)).sum::<f64>() / graded as f64;
            Self::tile(h, t.mean, &format!("{}%", exact_str(&mean, d)), "");
            let med = median(&pcts).map(|x| exact_str(&x, d)).unwrap_or_default();
            Self::tile(h, t.median, &format!("{med}%"), "");
            Self::tile(
                h,
                t.sd,
                &float_str(var.sqrt(), d as usize).unwrap_or_default(),
                "population SD",
            );
            let pass = rows.iter().filter(|r| r.passing == Some(true)).count();
            let rate = Exact::new(100 * pass as i128, graded as i128);
            Self::tile(
                h,
                t.pass_rate,
                &format!("{}%", exact_str(&rate, 1)),
                &format!("{pass} of {graded}"),
            );
        }
        if let Some(ts) = self.test {
            if let Some(a) = ts.alpha {
                let ci = ts
                    .alpha_ci
                    .and_then(|(l, u)| {
                        Some(format!(
                            "Feldt 95% CI {}–{}",
                            float_str(l, 2)?,
                            float_str(u, 2)?
                        ))
                    })
                    .unwrap_or_default();
                Self::tile(h, t.alpha, &float_str(a, 2).unwrap_or_default(), &ci);
            }
            if let Some(s) = ts.sem {
                Self::tile(
                    h,
                    t.sem,
                    &float_str(s, 2).unwrap_or_default(),
                    "raw-score points",
                );
            }
        }
        if let Some(x) = self.open_exceptions {
            Self::tile(h, t.open_exceptions, &x.to_string(), "");
        }
        h.push_str("</div>\n");
    }

    fn distribution(&self, h: &mut String) {
        let t = self.t();
        let bars = grade_distribution(self.results, self.scale);
        let total: usize = bars.iter().map(|b| b.count).sum();
        let max = bars.iter().map(|b| b.count).max().unwrap_or(0).max(1);
        let (w, top, base, bw, step, left) = (420.0, 24.0, 160.0, 28.0, 42.0, 22.0);
        let summary = bars
            .iter()
            .map(|b| format!("{} {}", b.letter, b.count))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(
            h,
            "<section class=\"card\" aria-labelledby=\"dist-h\">\n<h2 id=\"dist-h\">{}</h2>\n\
             <svg viewBox=\"0 0 {w} 214\" width=\"100%\" role=\"img\" aria-label=\"{}: {}\">\n\
             <defs><pattern id=\"hatch-fail\" width=\"6\" height=\"6\" patternUnits=\"userSpaceOnUse\" patternTransform=\"rotate(45)\">\
             <rect class=\"hatch-bg\" width=\"6\" height=\"6\"/><line class=\"hatch-line\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"6\"/></pattern></defs>\n",
            esc(t.distribution),
            esc(t.distribution),
            esc(&summary)
        );
        for k in 0..=4 {
            let y = base - (base - top) * f64::from(k) / 4.0;
            let _ = writeln!(
                h,
                "<line class=\"grid\" x1=\"{left}\" x2=\"{}\" y1=\"{y:.1}\" y2=\"{y:.1}\"/>",
                w - 4.0
            );
        }
        for (i, b) in bars.iter().enumerate() {
            let x = left + 8.0 + step * i as f64;
            let ht = (base - top) * b.count as f64 / max as f64;
            let y = base - ht;
            let fail = b.letter == "F";
            let class = if fail { "bar-fail" } else { "bar" };
            let cx = x + bw / 2.0;
            let _ = write!(
                h,
                "<g><title>{}: {}</title><rect class=\"{class}\" x=\"{x:.1}\" y=\"{y:.1}\" width=\"{bw}\" height=\"{ht:.1}\" rx=\"2\"/>\
                 <text class=\"count\" x=\"{cx:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text>\
                 <text x=\"{cx:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"12\" font-weight=\"600\">{}</text>\
                 <text class=\"ar\" x=\"{cx:.1}\" y=\"{:.1}\" text-anchor=\"middle\" direction=\"rtl\">{}</text>",
                esc(&b.letter),
                b.count,
                y - 4.0,
                b.count,
                base + 16.0,
                esc(&b.letter),
                base + 32.0,
                esc(&b.arabic),
            );
            if fail {
                let _ = write!(
                    h,
                    "<text class=\"fail-word\" x=\"{cx:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text>",
                    base + 47.0,
                    esc(t.fail)
                );
            }
            h.push_str("</g>\n");
        }
        let _ = writeln!(
            h,
            "<line class=\"axis\" x1=\"{left}\" x2=\"{}\" y1=\"{base}\" y2=\"{base}\"/>\n</svg>",
            w - 4.0
        );
        let _ = write!(
            h,
            "<div class=\"sub\">{} · GPA basis 5.00</div>\n<details><summary>{}</summary>\n\
             <table><thead><tr><th>{}</th><th></th><th class=\"num\">{}</th><th class=\"num\">{}</th></tr></thead><tbody>\n",
            esc(&self.scale.name),
            esc(t.table_view),
            esc(t.letter),
            esc(t.count),
            esc(t.percent)
        );
        for b in &bars {
            let p = if total == 0 {
                "0.0".to_string()
            } else {
                exact_str(&Exact::new(100 * b.count as i128, total as i128), 1)
            };
            let _ = writeln!(
                h,
                "<tr><td>{}</td><td lang=\"ar\" dir=\"auto\">{}</td><td class=\"num\">{}</td><td class=\"num\">{p}%</td></tr>",
                esc(&b.letter),
                esc(&b.arabic),
                b.count
            );
        }
        h.push_str("</tbody></table>\n</details>\n</section>\n");
    }

    fn clos(&self, h: &mut String) {
        let Some((res, set)) = self.clos else {
            return;
        };
        let t = self.t();
        let _ = write!(
            h,
            "<section class=\"card\" aria-labelledby=\"clo-h\">\n<h2 id=\"clo-h\">{}</h2>\n\
             <div class=\"legend\"><span><i class=\"sw\" style=\"background:var(--series-1)\"></i>Average μ</span>\
             <span><i class=\"sw\" style=\"background:var(--series-2)\"></i>Threshold-proportion π</span>\
             <span><i class=\"sw\" style=\"background:var(--ink)\"></i>{}</span></div>\n",
            esc(t.clo),
            esc(t.target)
        );
        for r in &res.clos {
            let title = set
                .clo(&r.clo)
                .map(|c| c.text_en.clone())
                .unwrap_or_default();
            let target = to_f(&r.target) * 100.0;
            let meter = |cls: &str, name: &str, v: Option<&Exact>, ci: Option<(f64, f64)>| {
                let (w, txt) = match v {
                    Some(x) => {
                        let p = to_f(x) * 100.0;
                        (
                            p.clamp(0.0, 100.0),
                            format!("{}%", exact_str(&(x * Exact::from_integer(100)), 1)),
                        )
                    }
                    None => (0.0, "—".into()),
                };
                // Proportions live in [0, 100] %: a t-interval is clamped for display.
                let ci_txt = ci
                    .and_then(|(l, u)| {
                        Some(format!(
                            " (95% CI {}–{})",
                            float_str((l * 100.0).clamp(0.0, 100.0), 1)?,
                            float_str((u * 100.0).clamp(0.0, 100.0), 1)?
                        ))
                    })
                    .unwrap_or_default();
                format!(
                    "<div class=\"mlabel\"><span>{name}</span><span class=\"mono\">{txt}{ci_txt}</span></div>\
                     <div class=\"meter\" role=\"img\" aria-label=\"{name} {txt}, {} {target:.0}%\"><div class=\"fill {cls}\" style=\"width:{w:.1}%\"></div>\
                     <div class=\"target\" style=\"inset-inline-start:{target:.1}%\"></div></div>",
                    esc(t.target)
                )
            };
            let verdict = match r.attained {
                Some(true) => format!("<span class=\"pill ok\">✓ {}</span>", esc(t.attained)),
                Some(false) => format!("<span class=\"pill bad\">◆ {}</span>", esc(t.not_attained)),
                None => "<span class=\"pill info\">Not assessed</span>".to_string(),
            };
            let badge = if r.indistinguishable {
                "<br><span class=\"pill warn\">± ≈ target (95% CI)</span>"
            } else {
                ""
            };
            let _ = writeln!(
                h,
                "<div class=\"clo\"><div><span class=\"mono\">{}</span> <span dir=\"auto\">{}</span><div class=\"sub\">verdict on {}</div></div>\
                 <div>{}{}</div><div>{verdict}{badge}</div></div>",
                esc(&r.clo),
                esc(&title),
                esc(r.method.name()),
                meter(
                    "mu",
                    "μ",
                    r.stats.mean.as_ref(),
                    r.stats.mean_bootstrap_ci.or(r.stats.mean_ci)
                ),
                meter(
                    "pi",
                    "π",
                    r.stats.proportion.as_ref(),
                    r.stats.proportion_ci
                ),
            );
        }
        h.push_str("</section>\n");
    }

    fn items(&self, h: &mut String) {
        let Some(rep) = self.items else {
            return;
        };
        let t = self.t();
        let _ = write!(
            h,
            "<section class=\"card\" aria-labelledby=\"items-h\">\n<h2 id=\"items-h\">{}</h2>\n",
            esc(t.items)
        );
        // Exam-level notes of the analysis (§12.3 small N, SCO-023; α, SCO-018).
        for e in rep
            .exceptions
            .iter()
            .filter(|e| e.code == "SCO-023" || e.code == "SCO-018")
        {
            let _ = writeln!(
                h,
                "<p class=\"sub\"><span class=\"mono\">{}</span> {}</p>",
                esc(&e.code),
                esc(&e.detail)
            );
        }
        let order = flag_order(rep);
        h.push_str(
            "<div class=\"scroll\"><table>\n\
             <thead><tr><th>Q</th><th class=\"num\">N</th><th class=\"num\">p</th><th class=\"num\">r<sub>ir</sub></th>\
             <th>Discrimination</th><th>Options</th><th class=\"num\">Flag order</th><th>Flags</th></tr></thead><tbody>\n",
        );
        for s in &rep.items {
            let rir = match s.r_ir.and_then(|v| float_str(v, 2).map(|x| (v, x))) {
                Some((v, x)) if v < 0.0 => format!("<span class=\"neg\">{x}</span>"),
                Some((_, x)) => x,
                None => "n/a".into(),
            };
            let band = s
                .discrimination
                .map(|b| {
                    let steps = match b {
                        Discrimination::ProbableFlaw => 0,
                        Discrimination::Review => 1,
                        Discrimination::Marginal => 2,
                        Discrimination::Acceptable => 3,
                        Discrimination::Good => 4,
                    };
                    let dots: String = (0..5).map(|k| if k <= steps { '●' } else { '○' }).collect();
                    format!("<span aria-hidden=\"true\">{dots}</span> {}", band_name(b))
                })
                .unwrap_or_default();
            // Option sparkline: one bar per option (keyed in series-1 when shown), omit last.
            let mut spark = String::from(
                "<svg width=\"84\" height=\"18\" viewBox=\"0 0 84 18\" role=\"img\" aria-label=\"option distribution\">",
            );
            let mut label = Vec::new();
            for (k, o) in s.options.iter().enumerate() {
                let hgt = (o.proportion * 16.0).clamp(0.0, 16.0);
                let fill = if self.show_key && o.keyed {
                    "var(--series-1)"
                } else {
                    "var(--ink-3)"
                };
                let _ = write!(
                    spark,
                    "<rect x=\"{}\" y=\"{:.1}\" width=\"9\" height=\"{hgt:.1}\" style=\"fill:{fill}\"/>",
                    k * 12,
                    17.0 - hgt
                );
                label.push(format!(
                    "{} {}",
                    o.option
                        .map_or("omit".to_string(), |x| option_code(u32::from(x))
                            .to_string()),
                    float_str(o.proportion, 2).unwrap_or_default()
                ));
            }
            spark.push_str("</svg>");
            let flags: Vec<String> = rep
                .flags
                .iter()
                .filter(|f| f.item == s.item)
                .map(|f| {
                    let cls = if f.is_confident_miskey() {
                        "bad"
                    } else if f.rank() <= 4 {
                        "warn"
                    } else {
                        "info"
                    };
                    format!("<span class=\"pill {cls}\">{}</span>", esc(&flag_label(f)))
                })
                .collect();
            let _ = writeln!(
                h,
                "<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{rir}</td><td>{band}</td>\
                 <td title=\"{}\">{spark}</td><td class=\"num\">{}</td><td>{}</td></tr>",
                esc(&s.label),
                s.n,
                s.p.and_then(|p| float_str(p, 2))
                    .unwrap_or_else(|| "n/a".into()),
                esc(&label.join(", ")),
                order
                    .get(&s.item)
                    .map(|k| k.to_string())
                    .unwrap_or_default(),
                flags.join(" ")
            );
        }
        h.push_str("</tbody></table></div>\n</section>\n");
    }

    fn students(&self, h: &mut String) {
        let t = self.t();
        let d = self.results.decimals;
        let hd = self.results.headers();
        let _ = write!(
            h,
            "<section class=\"card\" aria-labelledby=\"res-h\">\n<h2 id=\"res-h\">{}</h2>\n<div class=\"scroll\"><table>\n<thead><tr>\
             <th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th class=\"num\">{}</th><th class=\"num\">{}</th><th>{}</th><th>{}</th>\
             </tr></thead><tbody>\n",
            esc(t.results),
            esc(&hd[0]),
            esc(&hd[1]),
            esc(&hd[2]),
            esc(&hd[4]),
            esc(&hd[5]),
            esc(&hd[6]),
            esc(&hd[8]),
            esc(&hd[9]),
            esc(&hd[10])
        );
        for r in &self.results.rows {
            let score = match (&r.raw, &r.max) {
                (Some(a), Some(b)) => format!("{} / {}", exact_str(a, 2), exact_str(b, 2)),
                _ => String::new(),
            };
            let status = match r.status.as_str() {
                "scored" => String::new(),
                s => format!("<span class=\"pill warn\">{}</span>", esc(s)),
            };
            let letter_cls = if r.passing == Some(false) {
                " class=\"neg\""
            } else {
                ""
            };
            let _ = writeln!(
                h,
                "<tr><td class=\"id\" dir=\"ltr\">{}</td><td dir=\"auto\">{}</td><td dir=\"auto\">{}</td><td>{}</td><td>{status}</td>\
                 <td class=\"num\">{score}</td><td class=\"num\">{}</td><td{letter_cls}>{}</td><td lang=\"ar\" dir=\"auto\">{}</td></tr>",
                esc(r.student_id.as_deref().unwrap_or("")),
                esc(r.name.as_deref().unwrap_or("")),
                esc(r.section.as_deref().unwrap_or("")),
                esc(&r.form),
                r.percent
                    .as_ref()
                    .map(|x| exact_str(x, d))
                    .unwrap_or_default(),
                esc(r.letter.as_deref().unwrap_or("")),
                esc(r.letter_ar.as_deref().unwrap_or("")),
            );
        }
        h.push_str("</tbody></table></div>\n</section>\n");
    }

    fn footer(&self, h: &mut String) {
        let m = &self.results.meta;
        let _ = write!(
            h,
            "<footer class=\"prov\">\n<div class=\"classification\" role=\"note\">{LOCK}<span dir=\"auto\">{}</span></div>\n<dl>\n",
            esc(&m.classification.label())
        );
        for (k, v) in m.provenance() {
            let _ = writeln!(
                h,
                "<dt>{}</dt><dd class=\"mono\" dir=\"auto\">{}</dd>",
                esc(k),
                esc(&v)
            );
        }
        h.push_str("</dl>\n</footer>\n");
    }
}
