//! Answer keys and append-only key versions in master space (spec §10.1 "Key versions",
//! §10.3 "One key, in master space").

use crate::form::FormMap;
use crate::item::{ItemKind, MAX_OPTIONS, mask_of};
use crate::num::{Points, pts};
use crate::stats::chi2_sf;
use num_traits::{One, Zero};
use omr_model::{Exception, Scope, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The key entry of one master item.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyEntry {
    /// Single best answer / true–false: a credit proportion per master option (length m_i).
    /// Key = 1, optional partial (e.g. ½), 0, or an explicit negative −p (§10.4).
    Credits {
        /// c_o per master option.
        credits: Vec<Points>,
        /// Several full-credit options are intended (Remark "OR"); otherwise SCO-027 warns.
        multi_accept: bool,
    },
    /// Multi-select: the keyed set K as a mask over master options.
    Set(u32),
    /// Score box: no key; s = d (§10.4).
    Open,
}

impl KeyEntry {
    /// One keyed option out of `options`.
    pub fn single(option: u8, options: u8) -> Self {
        let mut credits = vec![Points::zero(); options as usize];
        if let Some(c) = credits.get_mut(option as usize) {
            *c = Points::one();
        }
        Self::Credits {
            credits,
            multi_accept: false,
        }
    }

    /// Several accepted answers (each full credit), explicitly multi-accepted.
    pub fn any_of(accepted: &[u8], options: u8) -> Self {
        let mut credits = vec![Points::zero(); options as usize];
        for &o in accepted {
            if let Some(c) = credits.get_mut(o as usize) {
                *c = Points::one();
            }
        }
        Self::Credits {
            credits,
            multi_accept: true,
        }
    }

    /// Sets a partial (or negative) credit on one option of a `Credits` entry.
    pub fn with_credit(mut self, option: u8, credit: Points) -> Self {
        if let Self::Credits { credits, .. } = &mut self
            && let Some(c) = credits.get_mut(option as usize)
        {
            *c = credit;
        }
        self
    }

    /// A keyed set for a multi-select item.
    pub fn set(keyed: &[u8]) -> Self {
        Self::Set(mask_of(keyed))
    }

    /// a_i: the number of full-credit options (|K| for a set).
    pub fn full_credit_count(&self) -> u32 {
        match self {
            Self::Credits { credits, .. } => {
                credits.iter().filter(|c| **c == Points::one()).count() as u32
            }
            Self::Set(m) => m.count_ones(),
            Self::Open => 0,
        }
    }

    /// The single keyed option when exactly one option has full credit.
    pub fn single_key(&self) -> Option<u8> {
        match self {
            Self::Credits { credits, .. } => {
                let mut it = credits
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| **c == Points::one());
                let first = it.next()?;
                if it.next().is_some() {
                    None
                } else {
                    Some(first.0 as u8)
                }
            }
            _ => None,
        }
    }

    /// Mask of the full-credit (keyed) options.
    pub fn keyed_mask(&self) -> u32 {
        match self {
            Self::Credits { credits, .. } => credits
                .iter()
                .enumerate()
                .filter(|(_, c)| **c == Points::one())
                .fold(0, |m, (o, _)| m | (1 << o)),
            Self::Set(m) => *m,
            Self::Open => 0,
        }
    }
}

/// An append-only key version in master space (§10.1). Per-form printed keys are derived
/// ([`KeyVersion::printed_key`]), never edited separately.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyVersion {
    /// Monotone version number.
    pub version: u32,
    /// Why this version exists (e.g. `KEY_ERROR` after a mis-key fix, §12.2).
    pub reason: String,
    /// One entry per master item; `None` = missing (SCO-003).
    pub entries: Vec<Option<KeyEntry>>,
}

impl KeyVersion {
    /// Version 1 from complete entries.
    pub fn new(entries: Vec<KeyEntry>) -> Self {
        Self {
            version: 1,
            reason: "initial".into(),
            entries: entries.into_iter().map(Some).collect(),
        }
    }

    /// A new version with `changes` applied (append-only: `self` is untouched). This is the
    /// `KeyVersionPublished` (L2) action, e.g. "change key to d" or "accept key ∪ {d}".
    pub fn revise(
        &self,
        changes: impl IntoIterator<Item = (usize, KeyEntry)>,
        reason: impl Into<String>,
    ) -> Self {
        let mut next = self.clone();
        next.version = self.version + 1;
        next.reason = reason.into();
        for (i, e) in changes {
            if i >= next.entries.len() {
                next.entries.resize(i + 1, None);
            }
            next.entries[i] = Some(e);
        }
        next
    }

    /// Entry of master item `i`.
    pub fn entry(&self, i: usize) -> Option<&KeyEntry> {
        self.entries.get(i).and_then(|e| e.as_ref())
    }

    /// The derived printed key of form `f`: per printed position, the entry in printed
    /// option space (credit of printed ℓ = c_{σ(ℓ)}).
    pub fn printed_key(&self, forms: &FormMap, f: usize) -> Vec<Option<KeyEntry>> {
        let form = &forms.forms[f];
        form.positions
            .iter()
            .enumerate()
            .map(|(j, p)| {
                let e = self.entry(p.item)?;
                Some(match e {
                    KeyEntry::Credits {
                        credits,
                        multi_accept,
                    } => KeyEntry::Credits {
                        credits: p
                            .option_map
                            .iter()
                            .map(|&o| credits.get(o as usize).copied().unwrap_or_default())
                            .collect(),
                        multi_accept: *multi_accept,
                    },
                    KeyEntry::Set(m) => KeyEntry::Set(form.unmap_mask(j, *m)),
                    KeyEntry::Open => KeyEntry::Open,
                })
            })
            .collect()
    }

    /// Publish checks (§10.1): SCO-003 (BLOCKER) for a missing or blank keyed item;
    /// SCO-004 (BLOCKER) for an entry that does not fit the item (option ≥ m_i, wrong type,
    /// credit > 1); SCO-027 (WARN) for a multi-mark SBA key without explicit multi-accept,
    /// a printed letter distribution with χ² p < 0.01, or a run of ≥ 5 identical letters.
    pub fn validate(&self, forms: &FormMap) -> Vec<Exception> {
        let mut ex = Vec::new();
        let sco003 =
            |d: String| Exception::must("SCO-003", Severity::Blocker, Scope::Exam).with_detail(d);
        let sco004 =
            |d: String| Exception::must("SCO-004", Severity::Blocker, Scope::Exam).with_detail(d);
        let sco027 =
            |d: String| Exception::must("SCO-027", Severity::Warn, Scope::Exam).with_detail(d);
        for (i, it) in forms.items.iter().enumerate() {
            let Some(e) = self.entry(i) else {
                ex.push(sco003(format!("item {} has no key entry", it.label)));
                continue;
            };
            match (it.kind, e) {
                (
                    ItemKind::SingleBestAnswer | ItemKind::TrueFalse,
                    KeyEntry::Credits {
                        credits,
                        multi_accept,
                    },
                ) => {
                    if credits.len() != it.options as usize {
                        ex.push(sco004(format!(
                            "item {}: {} credits for {} options",
                            it.label,
                            credits.len(),
                            it.options
                        )));
                        continue;
                    }
                    if credits.iter().any(|c| *c > Points::one()) {
                        ex.push(sco004(format!("item {}: a credit exceeds 1", it.label)));
                    }
                    if !credits.iter().any(|c| *c > Points::zero()) {
                        ex.push(sco003(format!(
                            "item {}: blank keyed item (no option earns credit)",
                            it.label
                        )));
                    }
                    if e.full_credit_count() > 1 && !multi_accept {
                        ex.push(sco027(format!(
                            "item {}: several full-credit options without explicit multi-accept",
                            it.label
                        )));
                    }
                }
                (ItemKind::MultiSelect(_), KeyEntry::Set(m)) => {
                    if *m == 0 {
                        ex.push(sco003(format!("item {}: empty multi-select key", it.label)));
                    } else if *m & !it.option_mask() != 0 {
                        ex.push(sco004(format!(
                            "item {}: key option ≥ m_i = {}",
                            it.label, it.options
                        )));
                    }
                }
                (ItemKind::ScoreBox, KeyEntry::Open) => {}
                (kind, _) => ex.push(sco004(format!(
                    "item {}: key entry type does not match {kind:?}",
                    it.label
                ))),
            }
        }
        // Printed-letter patterns per form (single-key SBA positions only).
        for pat in self.letter_patterns(forms) {
            ex.push(sco027(pat.detail()).with_sub_reason(pat.sub_reason()));
        }
        ex
    }
}

/// A printed-letter pattern of one form's key (SCO-027, §10.1 publish checks).
#[derive(Clone, Debug, PartialEq)]
pub enum LetterPattern {
    /// A run of `len` identical key letters ending at printed position `end` (1-based).
    Run {
        /// Form.
        form: omr_model::FormCode,
        /// Run length.
        len: usize,
        /// Last position of the run (1-based).
        end: usize,
    },
    /// A key letter distribution with χ² p < 0.01 over the items with `m` options.
    Distribution {
        /// Form.
        form: omr_model::FormCode,
        /// Key letters counted per option (A first).
        counts: Vec<u64>,
        /// Options of the items counted.
        m: u8,
        /// χ² statistic.
        chi2: f64,
        /// Its p-value.
        p: f64,
    },
}

impl LetterPattern {
    /// The form it is about.
    pub fn form(&self) -> omr_model::FormCode {
        match self {
            LetterPattern::Run { form, .. } | LetterPattern::Distribution { form, .. } => *form,
        }
    }

    /// Sub-reason of its SCO-027.
    pub fn sub_reason(&self) -> &'static str {
        match self {
            LetterPattern::Run { .. } => "letter_run",
            LetterPattern::Distribution { .. } => "letter_distribution",
        }
    }

    /// The full detail (the letter counts included), for the store and the owner's key
    /// summary.
    pub fn detail(&self) -> String {
        match self {
            LetterPattern::Run { form, len, end } => format!(
                "form {}: run of {len} identical key letters ending at position {end}",
                form.0
            ),
            LetterPattern::Distribution {
                form,
                counts,
                m,
                chi2,
                p,
            } => format!(
                "form {}: key letter distribution {counts:?} over {} items with {m} options is \
                 unlikely (χ² = {chi2:.2}, p = {p:.4})",
                form.0,
                counts.iter().sum::<u64>()
            ),
        }
    }

    /// A line safe for a terminal or a shared screen: no key letter and no letter counts,
    /// the version named by its label (C10). An option never keyed on items with `m` options
    /// suggests the exam uses fewer options than the sheet prints.
    pub fn public_text(&self, version: &str) -> String {
        match self {
            LetterPattern::Run { len, end, .. } => format!(
                "SCO-027 version {version}: {len} identical key letters in a row (ending at \
                 question {end}); check the key summary"
            ),
            LetterPattern::Distribution { counts, m, p, .. } => {
                let mut t = format!(
                    "SCO-027 version {version}: key letter distribution unlikely (p = {p:.3}); \
                     check the key summary"
                );
                if counts.contains(&0) && *m > 2 {
                    t.push_str(&format!(
                        "; one option is never keyed: if the exam uses {} options, grade with \
                         --options {}",
                        m - 1,
                        m - 1
                    ));
                }
                t
            }
        }
    }
}

impl KeyVersion {
    /// The printed-letter patterns of every form's key (single-key SBA positions only): runs of
    /// ≥ 5 identical letters and a letter distribution with χ² p < 0.01 per option count
    /// (expected count ≥ 5 per letter).
    pub fn letter_patterns(&self, forms: &FormMap) -> Vec<LetterPattern> {
        let mut out = Vec::new();
        for (f, form) in forms.forms.iter().enumerate() {
            let pk = self.printed_key(forms, f);
            let letters: Vec<Option<(u8, u8)>> = form
                .positions
                .iter()
                .zip(&pk)
                .map(|(p, e)| {
                    let it = forms.items.get(p.item)?;
                    if !it.kind.is_choice() {
                        return None;
                    }
                    Some((e.as_ref()?.single_key()?, it.options))
                })
                .collect();
            // Runs of ≥ 5 identical letters.
            let mut run = 0usize;
            let mut prev: Option<u8> = None;
            let mut flagged_run = false;
            for (j, l) in letters.iter().enumerate() {
                match l {
                    Some((k, _)) if prev == Some(*k) => run += 1,
                    Some((k, _)) => {
                        run = 1;
                        prev = Some(*k);
                    }
                    None => {
                        run = 0;
                        prev = None;
                    }
                }
                if run >= 5 && !flagged_run {
                    flagged_run = true;
                    out.push(LetterPattern::Run {
                        form: form.code,
                        len: run,
                        end: j + 1,
                    });
                }
            }
            // χ² letter distribution per option count m (expected count ≥ 5 per letter).
            let mut by_m: BTreeMap<u8, Vec<u64>> = BTreeMap::new();
            for (k, m) in letters.iter().flatten() {
                let counts = by_m.entry(*m).or_insert_with(|| vec![0; *m as usize]);
                if let Some(c) = counts.get_mut(*k as usize) {
                    *c += 1;
                }
            }
            for (m, counts) in by_m {
                let n: u64 = counts.iter().sum();
                let e = n as f64 / f64::from(m);
                if e < 5.0 {
                    continue;
                }
                let chi2: f64 = counts.iter().map(|&o| (o as f64 - e).powi(2) / e).sum();
                let p = chi2_sf(chi2, f64::from(m) - 1.0);
                if p < 0.01 {
                    out.push(LetterPattern::Distribution {
                        form: form.code,
                        counts,
                        m,
                        chi2,
                        p,
                    });
                }
            }
        }
        out
    }
}

/// Formula-scoring wrong credit −a_i/(m_i − a_i) (0 when a_i = m_i) (§10.5).
pub fn formula_wrong_credit(a: u32, m: u32) -> Points {
    if a >= m {
        Points::zero()
    } else {
        -pts(i64::from(a), i64::from(m - a))
    }
}

/// Option letter for display (A = 0).
pub fn letter(o: u8) -> char {
    if o < MAX_OPTIONS {
        (b'A' + o) as char
    } else {
        '?'
    }
}
