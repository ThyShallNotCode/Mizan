//! Analysis input: one score run's master-space choices and uncurved L3 item scores (§12).

use omr_scoring::response::cell;
use omr_scoring::{Counted, ItemKind, ItemState, ResponseMatrix, ScoreRun, num::points_f64};
use serde::{Deserialize, Serialize};

/// Status of one (examinee, item) cell for analysis (§12 "Data rules").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum CellStatus {
    /// Presented and reached: counts everywhere (omitted = 0).
    Reached = 0,
    /// Trailing blank run in a speeded section: missing for difficulty and option
    /// statistics, 0 for reliability.
    NotReached = 1,
    /// Not presented (other form, excused, unselected in a choice group): missing.
    NotPresented = 2,
}

/// Static facts about one analysed item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemInfo {
    /// Master item index in the score run (voided items are dropped from the analysis, so
    /// this can differ from the item's position in [`AnalysisData::items`]).
    pub master: usize,
    /// Label.
    pub label: String,
    /// Printed options m_i (0 for score boxes).
    pub options: u8,
    /// M_i.
    pub max: f64,
    /// Keyed (full-credit) master options.
    pub keyed: u32,
    /// Multi-select item (keyed set K).
    pub multi_select: bool,
    /// Single-answer item (SBA/TF): option statistics apply.
    pub choice: bool,
}

/// One item-set class's examinees × items, column-major (`v[i * n + e]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalysisData {
    /// Items (voided items are excluded: they leave α and totals).
    pub items: Vec<ItemInfo>,
    /// Examinees (absent students excluded).
    pub n: usize,
    /// u_i = s⁺/M_i ∈ [0, 1] (number-right credit; penalties never enter item analysis).
    pub u: Vec<f64>,
    /// x_i = graded item score (penalties included), for α, SEM, CSEM.
    pub x: Vec<f64>,
    /// Chosen master options (0 = omitted).
    pub choice: Vec<u32>,
    /// Cell status.
    pub status: Vec<CellStatus>,
    /// Any negative item score (penalties): KR-20/21 do not apply.
    pub penalties: bool,
    /// Score-run ID (seeds bootstrap draws).
    pub run_id: String,
}

impl AnalysisData {
    /// Builds the analysis input from a score run and its L2 matrix. Voided items and absent
    /// students are dropped; excused and unselected cells are "not presented".
    pub fn from_run(run: &ScoreRun, matrix: &ResponseMatrix) -> Self {
        let keep_items: Vec<usize> = (0..run.items.len())
            .filter(|&i| run.items[i].void.is_none())
            .collect();
        let rows: Vec<usize> = (0..run.n_rows())
            .filter(|&r| matrix.rows[r].absent.is_none())
            .collect();
        let n = rows.len();
        let k = keep_items.len();
        let mut u = vec![0.0; n * k];
        let mut x = vec![0.0; n * k];
        let mut choice = vec![0u32; n * k];
        let mut status = vec![CellStatus::NotPresented; n * k];
        let mut penalties = false;
        let mut items = Vec::with_capacity(k);
        for (j, &i) in keep_items.iter().enumerate() {
            let m = &run.items[i];
            let max = points_f64(&m.max_points);
            items.push(ItemInfo {
                master: i,
                label: m.label.clone(),
                options: m.options,
                max,
                keyed: m.keyed,
                multi_select: matches!(m.kind, ItemKind::MultiSelect(_)),
                choice: m.kind.is_choice(),
            });
            for (e, &r) in rows.iter().enumerate() {
                let idx = j * n + e;
                let st = run.item_state(r, i);
                let counted = matches!(run.counted(r, i), Counted::Counted);
                status[idx] = match (counted, st) {
                    (true, ItemState::NotReached) => CellStatus::NotReached,
                    (true, ItemState::Scored | ItemState::Blank | ItemState::Pending) => {
                        CellStatus::Reached
                    }
                    _ => CellStatus::NotPresented,
                };
                if status[idx] == CellStatus::NotPresented {
                    continue;
                }
                let s = points_f64(&run.item_score(r, i));
                if s < 0.0 {
                    penalties = true;
                }
                x[idx] = s;
                u[idx] = if max > 0.0 { s.max(0.0) / max } else { 0.0 };
                let c = matrix.raw(r, i);
                if cell::state(c) == cell::ANSWERED {
                    choice[idx] = cell::payload(c);
                }
            }
        }
        Self {
            items,
            n,
            u,
            x,
            choice,
            status,
            penalties,
            run_id: run.run_id.clone(),
        }
    }

    /// Dichotomous single-answer data for oracle tests: `keys[i]` is the keyed option of
    /// item i (M_i = 1, `options[i]` options); `answers[e][i]` the chosen option or `None`
    /// for an omit.
    pub fn from_choices(options: &[u8], keys: &[u8], answers: &[Vec<Option<u8>>]) -> Self {
        let n = answers.len();
        let k = keys.len();
        let mut u = vec![0.0; n * k];
        let mut choice = vec![0u32; n * k];
        for (e, row) in answers.iter().enumerate() {
            for i in 0..k {
                if let Some(o) = row.get(i).copied().flatten() {
                    choice[i * n + e] = 1 << o;
                    if o == keys[i] {
                        u[i * n + e] = 1.0;
                    }
                }
            }
        }
        Self {
            items: (0..k)
                .map(|i| ItemInfo {
                    master: i,
                    label: format!("Q{}", i + 1),
                    options: options[i],
                    max: 1.0,
                    keyed: 1 << keys[i],
                    multi_select: false,
                    choice: true,
                })
                .collect(),
            n,
            x: u.clone(),
            u,
            choice,
            status: vec![CellStatus::Reached; n * k],
            penalties: false,
            run_id: "oracle".into(),
        }
    }

    /// Cell index.
    #[inline]
    pub fn idx(&self, e: usize, i: usize) -> usize {
        i * self.n + e
    }
}
