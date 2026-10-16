//! The multi-form model: π_f (printed position → master item) and σ_f (printed option →
//! master option) (spec §10.3).

use crate::item::{ItemKind, MAX_OPTIONS, MasterItem};
use crate::num::Points;
use omr_model::{Exception, FormCode, Scope, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One printed position j of a form: π_f(j) and σ_{f,π_f(j)}.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FormPosition {
    /// Master item index π_f(j).
    pub item: usize,
    /// σ: printed option ℓ → master option (`option_map[ℓ]`); empty for score boxes.
    pub option_map: Vec<u8>,
}

/// A printed variant or per-student booklet (§10.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Form {
    /// Variant code from the page code (§6.4).
    pub code: FormCode,
    /// Printed positions in printed order.
    pub positions: Vec<FormPosition>,
}

impl Form {
    /// The identity form: printed order = master order, options unpermuted.
    pub fn identity(code: FormCode, items: &[MasterItem]) -> Self {
        Self {
            code,
            positions: items
                .iter()
                .enumerate()
                .map(|(i, it)| FormPosition {
                    item: i,
                    option_map: (0..it.options).collect(),
                })
                .collect(),
        }
    }

    /// A permuted form: `order[j]` = master item at printed position j and `option_maps[j]`
    /// = σ for that position (printed option → master option).
    pub fn permuted(code: FormCode, order: &[usize], option_maps: &[Vec<u8>]) -> Self {
        Self {
            code,
            positions: order
                .iter()
                .zip(option_maps)
                .map(|(&item, m)| FormPosition {
                    item,
                    option_map: m.clone(),
                })
                .collect(),
        }
    }

    /// Maps a printed option mask at position `j` to master space through σ (26-entry table).
    /// Entries of σ outside A–Z (an invalid form map, refused as SCO-004 at publish) and an
    /// unknown position map to nothing instead of overflowing the mask.
    pub fn map_mask(&self, j: usize, printed: u32) -> u32 {
        let Some(p) = self.positions.get(j) else {
            return 0;
        };
        let mut out = 0u32;
        let mut m = printed;
        while m != 0 {
            let l = m.trailing_zeros() as usize;
            m &= m - 1;
            if let Some(&o) = p.option_map.get(l)
                && o < MAX_OPTIONS
            {
                out |= 1 << o;
            }
        }
        out
    }

    /// Maps a master option mask back to printed space at position `j` (σ⁻¹).
    pub fn unmap_mask(&self, j: usize, master: u32) -> u32 {
        let Some(p) = self.positions.get(j) else {
            return 0;
        };
        let mut out = 0u32;
        for (l, &o) in p.option_map.iter().enumerate().take(MAX_OPTIONS as usize) {
            if o < MAX_OPTIONS && master & (1 << o) != 0 {
                out |= 1 << l;
            }
        }
        out
    }

    /// Printed position of each master item on this form (`None` if not presented).
    pub fn positions_of_items(&self, n_items: usize) -> Vec<Option<usize>> {
        let mut v = vec![None; n_items];
        for (j, p) in self.positions.iter().enumerate() {
            if let Some(slot) = v.get_mut(p.item) {
                *slot = Some(j);
            }
        }
        v
    }
}

/// Master items plus every form's maps (§10.3). One key, in master space, serves all forms.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FormMap {
    /// Master items in master order.
    pub items: Vec<MasterItem>,
    /// Forms (variants or booklets).
    pub forms: Vec<Form>,
    /// Per-form normalization declared (score as % of M_f); required when M_f differ (SCO-006).
    pub per_form_normalization: bool,
}

impl FormMap {
    /// One identity form `A` over `items`.
    pub fn single_form(items: Vec<MasterItem>) -> Self {
        let form = Form::identity(FormCode(1), &items);
        Self {
            items,
            forms: vec![form],
            per_form_normalization: false,
        }
    }

    /// Index of the form with `code`.
    pub fn form_index(&self, code: FormCode) -> Option<usize> {
        self.forms.iter().position(|f| f.code == code)
    }

    /// M_f = Σ_j M_{π_f(j)}.
    pub fn form_total(&self, f: usize) -> Points {
        self.forms[f]
            .positions
            .iter()
            .filter_map(|p| self.items.get(p.item))
            .map(|it| it.max_points)
            .sum()
    }

    /// Publish validation (§10.3): SCO-004 (BLOCKER) for an invalid item, a non-injective π_f,
    /// a σ that is not a bijection, a duplicated form code or a never-presented master item;
    /// SCO-006 (ERROR) when form totals differ without declared per-form normalization.
    pub fn validate(&self) -> Vec<Exception> {
        let mut ex = Vec::new();
        let sco004 = |detail: String| {
            Exception::must("SCO-004", Severity::Blocker, Scope::Exam).with_detail(detail)
        };
        for (i, it) in self.items.iter().enumerate() {
            let ok_opts = match it.kind {
                ItemKind::SingleBestAnswer => (2..=MAX_OPTIONS).contains(&it.options),
                ItemKind::TrueFalse => it.options == 2,
                ItemKind::MultiSelect(mode) => {
                    (2..=MAX_OPTIONS).contains(&it.options)
                        && match mode {
                            crate::item::SelectMode::PickN(n) => n >= 1 && n < it.options,
                            crate::item::SelectMode::SelectAll => true,
                        }
                }
                ItemKind::ScoreBox => it.options == 0,
            };
            if !ok_opts {
                ex.push(sco004(format!(
                    "item {} ({i}): option count {} invalid for {:?}",
                    it.label, it.options, it.kind
                )));
            }
            if it.max_points <= Points::from_integer(0) {
                ex.push(sco004(format!(
                    "item {} ({i}): maximum must be > 0",
                    it.label
                )));
            }
        }
        let mut codes = BTreeSet::new();
        let mut presented = vec![false; self.items.len()];
        for f in &self.forms {
            if !codes.insert(f.code) {
                ex.push(sco004(format!("form code {} duplicated", f.code.0)));
            }
            let mut seen = BTreeSet::new();
            for (j, p) in f.positions.iter().enumerate() {
                let Some(it) = self.items.get(p.item) else {
                    ex.push(sco004(format!(
                        "form {}: position {j} maps to unknown master item {}",
                        f.code.0, p.item
                    )));
                    continue;
                };
                if !seen.insert(p.item) {
                    ex.push(sco004(format!(
                        "form {}: π_f not injective (item {} printed twice)",
                        f.code.0, it.label
                    )));
                }
                presented[p.item] = true;
                let mut sorted = p.option_map.clone();
                sorted.sort_unstable();
                let identity: Vec<u8> = (0..it.options).collect();
                if sorted != identity {
                    ex.push(sco004(format!(
                        "form {}: σ at position {j} (item {}) is not a bijection on {} options",
                        f.code.0, it.label, it.options
                    )));
                }
            }
        }
        for (i, p) in presented.iter().enumerate() {
            if !p {
                ex.push(sco004(format!(
                    "master item {} is never presented",
                    self.items[i].label
                )));
            }
        }
        if !self.per_form_normalization && self.forms.len() > 1 {
            let totals: BTreeSet<Points> =
                (0..self.forms.len()).map(|f| self.form_total(f)).collect();
            if totals.len() > 1 {
                ex.push(
                    Exception::must("SCO-006", Severity::Error, Scope::Exam).with_detail(format!(
                        "form totals differ ({}) and no per-form normalization is declared",
                        totals
                            .iter()
                            .map(|t| t.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                );
            }
        }
        ex
    }
}
