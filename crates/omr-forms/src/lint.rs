//! Pre-print lint (spec §6.9), the layout-only rules.
//!
//! | Rule | Check | Level |
//! |---|---|---|
//! | L01 | Quiet zones (3 mm) around blocks, tags and codes; no printed element overlaps another | block |
//! | L02 | Bubble Ø < 20 px at the declared capture dpi | block |
//! | L03 | Horizontal pitch < 1.6 × Ø | warn |
//! | L04 | Content outside the safe box (§6.3) | block |
//! | L05 | Label or text collisions | block |
//! | L14 | `cancel_mode` unset | block |
//!
//! Every finding carries remediation text (R10).

use crate::geometry::{BUBBLE_DIAMETER, QUIET_ZONE};
use crate::layout::{LayoutManifest, PageArt, PageGeometry};
use crate::units::{RectUm, Um, um_to_px};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Capture resolution assumed by L02 unless another is declared (scanner floor, §7.2).
pub const DEFAULT_CAPTURE_DPI: u32 = 200;
/// Smallest bubble diameter in pixels (§6.3, §8.9).
pub const MIN_BUBBLE_PX: f64 = 20.0;

/// A lint rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LintRule {
    /// Quiet zones and overlaps.
    L01,
    /// Bubble size at capture resolution.
    L02,
    /// Bubble pitch.
    L03,
    /// Safe box.
    L04,
    /// Label collisions.
    L05,
    /// `cancel_mode` unset.
    L14,
}

/// Whether a finding blocks freeze and printing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LintLevel {
    /// Blocks freeze and printing.
    Block,
    /// Warns only.
    Warn,
}

/// One lint finding.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LintFinding {
    /// Rule.
    pub rule: LintRule,
    /// Level.
    pub level: LintLevel,
    /// 1-based page, if page-specific.
    pub page: Option<u16>,
    /// What is wrong and how to fix it (R10).
    pub message: String,
}

impl fmt::Display for LintFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.page {
            Some(p) => write!(f, "{:?} (page {p}): {}", self.rule, self.message),
            None => write!(f, "{:?}: {}", self.rule, self.message),
        }
    }
}

/// The L14 finding for an unset `cancel_mode`.
pub fn l14_cancel_mode_unset() -> LintFinding {
    LintFinding {
        rule: LintRule::L14,
        level: LintLevel::Block,
        page: None,
        message: "cancel_mode is unset. Fix: choose erase_only, cross_cancels or fill_cancels_check explicitly; there is no default because a wrong default changes scores (§8.8).".into(),
    }
}

#[derive(Clone, Debug)]
struct Element {
    name: String,
    rect: RectUm,
    quiet: bool,
}

fn elements(page: &PageGeometry, art: &PageArt) -> Vec<Element> {
    let mut v = Vec::new();
    for t in &page.tags {
        v.push(Element {
            name: format!("tag {}", t.id),
            rect: t.outer,
            quiet: true,
        });
    }
    for c in &page.codes {
        v.push(Element {
            name: format!("page code {}", c.copy),
            rect: c.rect,
            quiet: true,
        });
        v.push(Element {
            name: format!("echo line {}", c.copy),
            rect: c.echo,
            quiet: false,
        });
    }
    for b in &page.blocks {
        v.push(Element {
            name: format!("block {:?}", b.kind),
            rect: b.rect,
            quiet: true,
        });
    }
    let in_block = |r: &RectUm| page.blocks.iter().any(|b| b.rect.contains_rect(r));
    for t in &art.texts {
        if in_block(&t.bbox) {
            continue;
        }
        v.push(Element {
            name: format!("text {:?} {:?}", t.role, t.content),
            rect: t.bbox,
            quiet: false,
        });
    }
    for r in &art.rules {
        v.push(Element {
            name: "write-in rule".into(),
            rect: RectUm::from_ltrb(r.from.x, r.from.y - r.width, r.to.x, r.to.y + r.width),
            quiet: false,
        });
    }
    for f in &art.frames {
        if !in_block(f) {
            v.push(Element {
                name: "comb box".into(),
                rect: *f,
                quiet: false,
            });
        }
    }
    for e in &art.examples {
        v.push(Element {
            name: format!("example {:?}", e.kind),
            rect: e.rect(),
            quiet: false,
        });
    }
    v
}

/// Runs the layout lint rules over a manifest and its art.
pub fn lint_layout(m: &LayoutManifest, arts: &[PageArt], capture_dpi: u32) -> Vec<LintFinding> {
    let mut out = Vec::new();
    // L02: bubble size in pixels at the declared capture resolution.
    let px = um_to_px(m.bubble.diameter, f64::from(capture_dpi));
    if px < MIN_BUBBLE_PX {
        out.push(LintFinding {
            rule: LintRule::L02,
            level: LintLevel::Block,
            page: None,
            message: format!(
                "bubble Ø is {px:.1} px at {capture_dpi} dpi (< {MIN_BUBBLE_PX}). Fix: scan at a higher resolution or enlarge bubbles."
            ),
        });
    }
    let empty = PageArt::default();
    for page in &m.pages {
        let art = arts.get(usize::from(page.page) - 1).unwrap_or(&empty);
        lint_page(m, page, art, &mut out);
    }
    out
}

fn lint_page(m: &LayoutManifest, page: &PageGeometry, art: &PageArt, out: &mut Vec<LintFinding>) {
    let p = Some(page.page);
    let els = elements(page, art);
    // L04: everything inside the safe box.
    for e in &els {
        if !m.safe_box.contains_rect(&e.rect) {
            out.push(LintFinding {
                rule: LintRule::L04,
                level: LintLevel::Block,
                page: p,
                message: format!(
                    "{} at {:?} lies outside the safe box. Fix: move it inside x 7–203 mm, y 24.6–272.4 mm (§6.3).",
                    e.name, e.rect
                ),
            });
        }
    }
    // L01 / L05: quiet zones and overlaps between distinct elements.
    for (i, a) in els.iter().enumerate() {
        for b in els.iter().skip(i + 1) {
            let overlap = a.rect.intersects(&b.rect);
            let quiet = (a.quiet && a.rect.inflate(QUIET_ZONE).intersects(&b.rect))
                || (b.quiet && b.rect.inflate(QUIET_ZONE).intersects(&a.rect));
            if overlap && !a.quiet && !b.quiet {
                out.push(LintFinding {
                    rule: LintRule::L05,
                    level: LintLevel::Block,
                    page: p,
                    message: format!(
                        "{} collides with {}. Fix: shorten or move one of them.",
                        a.name, b.name
                    ),
                });
            } else if quiet {
                out.push(LintFinding {
                    rule: LintRule::L01,
                    level: LintLevel::Block,
                    page: p,
                    message: format!(
                        "{} intrudes on the 3 mm quiet zone of {}. Fix: move it at least 3 mm away (§6.3).",
                        b.name, a.name
                    ),
                });
            }
        }
    }
    // L01 inside blocks: bubbles never overlap each other, ticks or question numbers, and
    // stay inside their block.
    let mut circles: Vec<(String, crate::units::PtUm, Um)> = page
        .bubbles
        .iter()
        .map(|b| (b.bubble_id(), b.centre, b.radius))
        .collect();
    circles.extend(
        page.id_bubbles
            .iter()
            .map(|b| (format!("ID{}/{}", b.column, b.digit), b.centre, b.radius)),
    );
    circles.extend(
        page.form_bubbles
            .iter()
            .map(|b| (format!("FORM/{}", b.form), b.centre, b.radius)),
    );
    for (i, a) in circles.iter().enumerate() {
        let ra = RectUm::around(a.1, a.2);
        if !page.blocks.iter().any(|b| b.rect.contains_rect(&ra)) {
            out.push(LintFinding {
                rule: LintRule::L01,
                level: LintLevel::Block,
                page: p,
                message: format!("bubble {} lies outside every block. Fix: regenerate the layout.", a.0),
            });
        }
        for b in circles.iter().skip(i + 1) {
            let min = i64::from(a.2 + b.2);
            if a.1.dist2(b.1) < min * min {
                out.push(LintFinding {
                    rule: LintRule::L01,
                    level: LintLevel::Block,
                    page: p,
                    message: format!("bubbles {} and {} overlap. Fix: increase the pitch.", a.0, b.0),
                });
            }
        }
        for t in &page.ticks {
            if t.rect.intersects(&ra) {
                out.push(LintFinding {
                    rule: LintRule::L01,
                    level: LintLevel::Block,
                    page: p,
                    message: format!("row tick of Q{} touches bubble {}. Fix: widen the row.", t.question + 1, a.0),
                });
            }
        }
        for t in &art.texts {
            if t.bbox.intersects(&ra) {
                out.push(LintFinding {
                    rule: LintRule::L05,
                    level: LintLevel::Block,
                    page: p,
                    message: format!("text {:?} collides with bubble {}. Fix: move the text.", t.content, a.0),
                });
            }
        }
    }
    // L03: horizontal pitch within a question.
    for w in page.bubbles.windows(2) {
        if w[0].question == w[1].question {
            let pitch = (w[1].centre.x - w[0].centre.x).abs();
            if pitch * 10 < 16 * BUBBLE_DIAMETER {
                out.push(LintFinding {
                    rule: LintRule::L03,
                    level: LintLevel::Warn,
                    page: p,
                    message: format!(
                        "Q{} pitch {pitch} µm is below 1.6 × Ø. Fix: use a 7.0 mm pitch.",
                        w[0].question + 1
                    ),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Template;
    use crate::spec::TemplateSpec;
    use omr_model::{CancelMode, ExamId};

    #[test]
    fn standard_layouts_are_clean() {
        for (q, o, id) in [(20, 4, 9), (60, 5, 10), (100, 5, 10), (150, 4, 0), (30, 10, 12)] {
            let mut s = TemplateSpec::standard(ExamId(uuid::Uuid::from_u128(1)), q, o, CancelMode::EraseOnly);
            s.id_digits = id;
            s.form_grid = true;
            s.forms = 4;
            let t = Template::build(s).unwrap();
            let f = t.lint_at_dpi(200);
            assert!(f.is_empty(), "{q}x{o}: {f:#?}");
        }
    }

    #[test]
    fn l02_flags_low_capture_dpi() {
        let t = Template::build(TemplateSpec::standard(
            ExamId(uuid::Uuid::from_u128(1)),
            10,
            4,
            CancelMode::EraseOnly,
        ))
        .unwrap();
        assert!(t.lint_at_dpi(100).iter().any(|f| f.rule == LintRule::L02));
        assert!(t.lint_at_dpi(150).is_empty());
    }

    #[test]
    fn l01_and_l04_catch_a_moved_bubble() {
        let t = Template::build(TemplateSpec::standard(
            ExamId(uuid::Uuid::from_u128(1)),
            10,
            4,
            CancelMode::EraseOnly,
        ))
        .unwrap();
        let mut m = t.manifest().clone();
        // Push question 1's option 1 onto option 0, and a tag outside the safe box.
        let c0 = m.pages[0].bubbles[0].centre;
        m.pages[0].bubbles[1].centre = c0;
        m.pages[0].tags[0].outer.x -= 2_000;
        let f = lint_layout(&m, t.arts(), 200);
        assert!(f.iter().any(|x| x.rule == LintRule::L01 && x.message.contains("overlap")));
        assert!(f.iter().any(|x| x.rule == LintRule::L04));
        assert!(f.iter().all(|x| x.message.contains("Fix:")), "R10 remediation text");
    }
}
