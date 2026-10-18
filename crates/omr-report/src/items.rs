//! Item analysis export (spec §17.2 "Test summary", "Item analysis"; §12.1–§12.3) from
//! `omr-analytics`. Advisory statistics, three decimals; the keyed option is hidden unless the
//! caller asks for it (key secrecy, §19.3; the design hides it by default).

use crate::error::ReportError;
use crate::meta::ReportMeta;
use crate::table::{Cell, Row, Sheet, write_csv, write_xlsx};
use crate::text::{float_str, option_code};
use omr_analytics::{
    AlphaBand, AnalyticsReport, Confidence, Discrimination, ItemFlag, ItemStats, Rule, SmallN,
    TestStats,
};
use std::io::Write;

/// The item-analysis export.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemAnalysis<'a> {
    /// Header and provenance.
    pub meta: &'a ReportMeta,
    /// The analytics report.
    pub report: &'a AnalyticsReport,
    /// Show which option is keyed.
    pub show_key: bool,
}

/// Short name of a small-N tier.
pub fn tier_name(t: SmallN) -> &'static str {
    match t {
        SmallN::Minimal => "minimal (N<10)",
        SmallN::Low => "low (10-29)",
        SmallN::Limited => "limited (30-49)",
        SmallN::Moderate => "moderate (50-99)",
        SmallN::Full => "full",
    }
}

/// Band word of a discrimination value (§9.5 design: 5-step meter + band word).
pub fn band_name(b: Discrimination) -> &'static str {
    match b {
        Discrimination::ProbableFlaw => "probable flaw",
        Discrimination::Review => "review",
        Discrimination::Marginal => "marginal",
        Discrimination::Acceptable => "acceptable",
        Discrimination::Good => "good",
    }
}

/// Reliability band name.
pub fn alpha_band_name(b: AlphaBand) -> &'static str {
    match b {
        AlphaBand::HighStakes => "high stakes (>= .90)",
        AlphaBand::EndOfCourse => "end of course (.80-.89)",
        AlphaBand::LowStakes => "low stakes (.70-.79)",
        AlphaBand::Below => "below .70",
    }
}

/// A flag as a short label with the spec's message (§12.2), e.g. `SCO-001 Probable mis-key:
/// D`; with its flag stability once the job has run: `SCO-002 Probable mis-key: D (proposed)
/// [fires in 31 % of bootstrap resamples]` (a low-confidence rank-1 flag is the non-blocking
/// SCO-002 `proposed_miskey`, [`ItemFlag::code`]).
pub fn flag_label(f: &ItemFlag) -> String {
    let code = f.code().map(|c| format!("{c} ")).unwrap_or_default();
    let conf = match f.confidence {
        Confidence::Normal => "",
        Confidence::Low => " (proposed)",
        Confidence::InsufficientData => " (possible, insufficient data)",
    };
    let stability = f
        .stability
        .map(|s| format!(" [fires in {:.0} % of bootstrap resamples]", s * 100.0))
        .unwrap_or_default();
    format!("{code}{}{conf}{stability}", f.rule.message(f.option))
}

/// The 1-based place of each item's highest flag of ranks 1–4 in the ranked list (§12.2: by
/// rank, then stability, then item), by data item; items without such a flag are absent.
pub fn flag_order(report: &AnalyticsReport) -> std::collections::BTreeMap<usize, usize> {
    let mut out = std::collections::BTreeMap::new();
    for f in report.flags.iter().filter(|f| f.rank() <= 4) {
        let next = out.len() + 1;
        out.entry(f.item).or_insert(next);
    }
    out
}

fn f3(x: Option<f64>) -> Cell {
    Cell::num(x.and_then(|v| float_str(v, 3)), 3)
}

impl ItemAnalysis<'_> {
    fn max_options(&self) -> usize {
        self.report
            .items
            .iter()
            .flat_map(|s| s.options.iter())
            .filter_map(|o| o.option)
            .map(|o| usize::from(o) + 1)
            .max()
            .unwrap_or(0)
    }

    /// Per-item statistics sheet.
    pub fn items_sheet(&self) -> Sheet {
        let k = self.max_options();
        let mut header: Vec<String> = [
            "Item",
            "N",
            "Tier",
            "p",
            "D",
            "r_pb",
            "r_ir",
            "r_ir CI low",
            "r_ir CI high",
            "r_ir bootstrap CI low",
            "r_ir bootstrap CI high",
            "Discrimination",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        header.extend((0..k).map(|o| format!("P({})", option_code(o as u32))));
        header.push("P(omit)".into());
        if self.show_key {
            header.push("Key".into());
        }
        header.push("Flag order".into());
        header.push("Flags".into());
        let order = flag_order(self.report);
        let boot = self.report.stability.as_ref();
        let rows = self
            .report
            .items
            .iter()
            .map(|s: &ItemStats| {
                let mut c = vec![
                    Cell::text(s.label.clone()),
                    Cell::Int(s.n as i64),
                    Cell::text(tier_name(s.tier)),
                    f3(s.p),
                    f3(s.d_index),
                    f3(s.r_pb),
                    f3(s.r_ir),
                    f3(s.r_ir_ci.map(|x| x.0)),
                    f3(s.r_ir_ci.map(|x| x.1)),
                    f3(boot.and_then(|b| b.r_ir_ci(s.item)).map(|x| x.0)),
                    f3(boot.and_then(|b| b.r_ir_ci(s.item)).map(|x| x.1)),
                    Cell::opt_text(s.discrimination.map(band_name)),
                ];
                for o in 0..k {
                    let p = s
                        .options
                        .iter()
                        .find(|x| x.option.map(usize::from) == Some(o))
                        .map(|x| x.proportion);
                    c.push(f3(p));
                }
                c.push(f3(s
                    .options
                    .iter()
                    .find(|x| x.option.is_none())
                    .map(|x| x.proportion)));
                if self.show_key {
                    let keyed: String = s
                        .options
                        .iter()
                        .filter(|x| x.keyed)
                        .filter_map(|x| x.option)
                        .map(|o| option_code(u32::from(o)))
                        .collect();
                    c.push(Cell::text(keyed));
                }
                let flags: Vec<String> = self
                    .report
                    .flags
                    .iter()
                    .filter(|f| f.item == s.item)
                    .map(flag_label)
                    .collect();
                c.push(match order.get(&s.item) {
                    Some(&k) => Cell::Int(k as i64),
                    None => Cell::text(""),
                });
                c.push(Cell::text(flags.join("; ")));
                Row::new(c)
            })
            .collect();
        let mut widths = vec![
            8.0, 6.0, 16.0, 7.0, 7.0, 7.0, 7.0, 9.0, 9.0, 11.0, 11.0, 14.0,
        ];
        widths.extend(std::iter::repeat_n(7.0, k + 1));
        if self.show_key {
            widths.push(5.0);
        }
        widths.push(7.0);
        widths.push(50.0);
        Sheet {
            name: "Items".into(),
            title: format!("{} — item analysis", self.meta.title),
            headers: vec![header],
            rows,
            notes: vec![
                "p = difficulty; D = upper-lower 27% (hidden below N = 50); r_pb uncorrected \
                 point-biserial; r_ir corrected item-rest r with Fisher-z 95% CI (§12.1) and, \
                 below N = 1,000, the percentile 95% CI over 1,000 bootstrap resamples (§12.3; \
                 blank when r_ir is undefined in more than 10% of them)."
                    .into(),
                "Flags: §12.2 messages; flag order = rank, then stability. A probable mis-key \
                 (SCO-001) must be acknowledged before release; one marked (proposed) is low \
                 confidence (SCO-002, not blocking)."
                    .into(),
                "Advisory statistics: nothing here changes a score (§12).".into(),
            ],
            rtl: false,
            widths,
        }
    }

    /// Test-summary sheet (one row per item-set class).
    pub fn tests_sheet(&self) -> Sheet {
        let header: Vec<String> = [
            "Items",
            "N",
            "Mean",
            "Median",
            "SD",
            "Min",
            "Max",
            "P10",
            "P25",
            "P75",
            "P90",
            "KR-20",
            "KR-21",
            "Alpha",
            "Alpha CI low",
            "Alpha CI high",
            "SEM",
            "Band",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let rows = self
            .report
            .tests
            .iter()
            .map(|t: &TestStats| {
                let s = &t.summary;
                Row::new(vec![
                    Cell::Int(t.items.len() as i64),
                    Cell::Int(t.n as i64),
                    f3(Some(s.mean)),
                    f3(Some(s.median)),
                    f3(s.sd),
                    f3(Some(s.min)),
                    f3(Some(s.max)),
                    f3(Some(s.percentiles[0])),
                    f3(Some(s.percentiles[1])),
                    f3(Some(s.percentiles[2])),
                    f3(Some(s.percentiles[3])),
                    f3(t.kr20),
                    f3(t.kr21),
                    f3(t.alpha),
                    f3(t.alpha_ci.map(|x| x.0)),
                    f3(t.alpha_ci.map(|x| x.1)),
                    f3(t.sem),
                    Cell::opt_text(t.band.map(alpha_band_name)),
                ])
            })
            .collect();
        Sheet {
            name: "Test summary".into(),
            title: format!("{} — test summary", self.meta.title),
            headers: vec![header],
            rows,
            notes: vec![
                "Population variance (÷N); α with the Feldt 95% CI; SEM = σ·√(1 − α).".into(),
            ],
            rtl: false,
            widths: vec![7.0; 18],
        }
    }

    /// Ranked flags sheet (§12.2).
    pub fn flags_sheet(&self) -> Sheet {
        let header = [
            "Item",
            "Rank",
            "Code",
            "Rule",
            "Option",
            "Confidence",
            "Stability",
            "Message",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let rows = self
            .report
            .flags
            .iter()
            .map(|f| {
                Row::new(vec![
                    Cell::text(f.label.clone()),
                    Cell::Int(i64::from(f.rank())),
                    Cell::opt_text(f.code()),
                    Cell::text(f.rule.message(f.option)),
                    Cell::opt_text(f.option.map(|o| option_code(u32::from(o)).to_string())),
                    Cell::text(match f.confidence {
                        Confidence::Normal => "normal",
                        Confidence::Low => "low (proposed)",
                        Confidence::InsufficientData => "possible, insufficient data",
                    }),
                    f3(f.stability),
                    // These messages name the keyed option ("… B keyed, D not"; "two correct
                    // answers: B and D"): hidden with the key (§19.3).
                    if !self.show_key && matches!(f.rule, Rule::MiskeyedOption | Rule::TwoCorrect) {
                        Cell::text("(names the keyed option: shown only with the key)")
                    } else {
                        Cell::text(f.message.clone())
                    },
                ])
            })
            .collect();
        Sheet {
            name: "Flags".into(),
            title: format!("{} — mis-key and flawed-item flags", self.meta.title),
            headers: vec![header],
            rows,
            notes: vec![],
            rtl: false,
            widths: vec![8.0, 6.0, 9.0, 24.0, 7.0, 16.0, 9.0, 70.0],
        }
    }

    /// Item statistics as CSV.
    pub fn write_items_csv<W: Write>(&self, w: &mut W) -> Result<(), ReportError> {
        write_csv(w, &self.items_sheet())
    }

    /// Test summary as CSV.
    pub fn write_tests_csv<W: Write>(&self, w: &mut W) -> Result<(), ReportError> {
        write_csv(w, &self.tests_sheet())
    }

    /// Items, test summary and flags as one XLSX workbook.
    pub fn write_xlsx<W: Write>(&self, w: &mut W) -> Result<(), ReportError> {
        write_xlsx(
            w,
            self.meta,
            &[self.items_sheet(), self.tests_sheet(), self.flags_sheet()],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(item: usize, rule: Rule, confidence: Confidence, stability: Option<f64>) -> ItemFlag {
        ItemFlag {
            item,
            master: item,
            label: format!("Q{}", item + 1),
            rule,
            option: Some(3),
            message: String::new(),
            confidence,
            stability,
        }
    }

    #[test]
    fn flag_labels_carry_the_spec_messages_and_the_proposed_label() {
        // C12: the spec's messages (§12.2), never Rust enum names; a low-confidence rank-1
        // flag is the non-blocking SCO-002 and reads "(proposed)".
        let f = flag(0, Rule::ProbableMiskey, Confidence::Normal, Some(1.0));
        assert_eq!(
            flag_label(&f),
            "SCO-001 Probable mis-key: D [fires in 100 % of bootstrap resamples]"
        );
        let f = flag(0, Rule::ProbableMiskey, Confidence::Low, Some(0.31));
        assert_eq!(
            flag_label(&f),
            "SCO-002 Probable mis-key: D (proposed) [fires in 31 % of bootstrap resamples]"
        );
        let f = flag(2, Rule::TopGroupPrefers, Confidence::Normal, None);
        assert_eq!(flag_label(&f), "SCO-002 Top group prefers D");
        let f = flag(2, Rule::TwoCorrect, Confidence::Normal, None);
        assert_eq!(flag_label(&f), "SCO-002 Possibly two correct answers (D)");
        let f = flag(2, Rule::LowDiscrimination, Confidence::Normal, None);
        assert_eq!(flag_label(&f), "Low discrimination");
        for r in [
            Rule::ProbableMiskey,
            Rule::TopGroupPrefers,
            Rule::TwoCorrect,
            Rule::NonFunctioningDistractor,
        ] {
            assert!(!flag_label(&flag(0, r, Confidence::Normal, None)).contains(&format!("{r:?}")));
        }
    }

    #[test]
    fn flag_order_follows_rank_then_the_flags_order() {
        let report = AnalyticsReport {
            items: vec![],
            tests: vec![],
            // Already ranked by the analysis: rank, then stability, then item.
            flags: vec![
                flag(4, Rule::ProbableMiskey, Confidence::Normal, Some(0.99)),
                flag(1, Rule::ProbableMiskey, Confidence::Low, Some(0.40)),
                flag(4, Rule::TopGroupPrefers, Confidence::Normal, Some(0.9)),
                flag(7, Rule::TwoCorrect, Confidence::Normal, Some(0.7)),
                flag(9, Rule::LowDiscrimination, Confidence::Normal, None),
            ],
            exceptions: vec![],
            stability: None,
        };
        let o = flag_order(&report);
        assert_eq!(o.get(&4), Some(&1));
        assert_eq!(o.get(&1), Some(&2));
        assert_eq!(o.get(&7), Some(&3));
        assert_eq!(o.get(&9), None, "rank 5–6 notes are not ordered");
    }
}
