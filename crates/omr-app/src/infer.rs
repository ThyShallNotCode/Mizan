//! Key and form inference (§10.3).
//!
//! - **Inferred key** (explicit opt-in): per form, per question, the modal *confident*
//!   (auto-accepted single) response of the booklets whose form is declared confidently (on a
//!   one-form exam: every booklet with a form).
//!   Ties go to the lower option; a question with no confident response is voided.
//! - **Form inference**: LL_f(n) = Σ_j log p̂_{π_f(j)}(r_{n,j}) over the categories options +
//!   blank + multi, with p̂ from the confirmed booklets of the form when it has ≥ 20 of them
//!   (leave-one-out for a booklet that declares the form), else the small-class fallback built
//!   from the form's key. Decision table: declared =
//!   argmax → accept; declared ≠ argmax with ΔLL ≥ 10 nats → IDN-003 (ERROR, never
//!   auto-switch); declared ≠ argmax below 10 nats → keep declared; nothing declared →
//!   provisional argmax + SCO-005 `inferred` (ERROR).

use crate::model::{Booklet, FormInference, KeyCell, ResponseStatus};
use omr_model::exceptions::Target;
use omr_model::{Exception, Response, Scope, Severity};

/// Margin for IDN-003 and group confirmation (nats; heuristic, §10.3).
pub const DELTA_LL_NATS: f64 = 10.0;

/// Category of a response: option index, `m` = blank, `m + 1` = multi; `None` = unusable.
fn category(r: &Response, m: usize) -> Option<usize> {
    match r {
        Response::Single(o) if usize::from(*o) < m => Some(usize::from(*o)),
        Response::Blank => Some(m),
        Response::Multi(_) | Response::Set(_) => Some(m + 1),
        _ => None,
    }
}

/// A response usable as evidence: auto-accepted or reviewed.
fn evidence(b: &Booklet, q: usize) -> Option<&Response> {
    let s = b.questions.get(q)?;
    (s.status != ResponseStatus::Pending).then_some(&s.current)
}

/// The modal confident response per form and question (`forms × questions` cells).
pub fn infer_key(
    booklets: &[Booklet],
    n_forms: usize,
    questions: usize,
    options: u8,
) -> Vec<Vec<KeyCell>> {
    let m = usize::from(options);
    (0..n_forms)
        .map(|f| {
            (0..questions)
                .map(|q| {
                    let mut counts = vec![0u32; m];
                    // The booklets of form f before inference: declared by code or bubble, or
                    // every booklet of a one-form exam (whose version bubble is often blank).
                    for b in booklets.iter().filter(|b| {
                        b.declared_form == Some(f) || (n_forms == 1 && b.form == Some(f))
                    }) {
                        if let Some(Response::Single(o)) = evidence(b, q)
                            && usize::from(*o) < m
                            && b.questions[q].status == ResponseStatus::Auto
                        {
                            counts[usize::from(*o)] += 1;
                        }
                    }
                    let best = counts.iter().copied().max().unwrap_or(0);
                    if best == 0 {
                        KeyCell::Void
                    } else {
                        let o = counts.iter().position(|&c| c == best).unwrap_or(0);
                        KeyCell::Options(vec![o as u8])
                    }
                })
                .collect()
        })
        .collect()
}

/// The response model of one form (§10.3).
enum FormModel {
    /// Empirical category counts per question (`counts[q][c]`, `n[q]`) from the booklets that
    /// declare the form (≥ 20 of them); evaluated leave-one-out for those booklets.
    Empirical { counts: Vec<Vec<u32>>, n: Vec<u32> },
    /// The small-class fallback built from the key: `lp[q][c]`.
    Fallback(Vec<Vec<f64>>),
}

/// Per-form response models.
fn model(
    booklets: &[Booklet],
    keys: &[Vec<KeyCell>],
    questions: usize,
    options: u8,
) -> Vec<FormModel> {
    let m = usize::from(options);
    let k = m + 2;
    // Class-wide blank/multi rates and proportion correct (all booklets with a known form).
    let mut blanks = vec![0u32; questions];
    let mut multis = vec![0u32; questions];
    let mut seen = vec![0u32; questions];
    let (mut correct, mut answered) = (0u64, 0u64);
    for b in booklets {
        for q in 0..questions {
            if let Some(c) = evidence(b, q).and_then(|r| category(r, m)) {
                seen[q] += 1;
                if c == m {
                    blanks[q] += 1;
                } else if c == m + 1 {
                    multis[q] += 1;
                } else if let Some(f) = b.declared_form {
                    answered += 1;
                    if matches!(keys.get(f).and_then(|kf| kf.get(q)), Some(KeyCell::Options(v)) if v.contains(&(c as u8)))
                    {
                        correct += 1;
                    }
                }
            }
        }
    }
    let p_bar = if answered == 0 {
        0.7
    } else {
        correct as f64 / answered as f64
    }
    .clamp(1.0 / m as f64 + 0.05, 0.95);
    (0..keys.len())
        .map(|f| {
            let conf: Vec<&Booklet> = booklets
                .iter()
                .filter(|b| b.declared_form == Some(f))
                .collect();
            if conf.len() >= 20 {
                let mut counts = vec![vec![0u32; k]; questions];
                let mut n = vec![0u32; questions];
                for b in &conf {
                    for q in 0..questions {
                        if let Some(cat) = evidence(b, q).and_then(|r| category(r, m)) {
                            counts[q][cat] += 1;
                            n[q] += 1;
                        }
                    }
                }
                return FormModel::Empirical { counts, n };
            }
            FormModel::Fallback(
                (0..questions)
                    .map(|q| {
                        let n = f64::from(seen[q]);
                        let pb = (f64::from(blanks[q]) + 0.5) / (n + 1.0);
                        let pm = (f64::from(multis[q]) + 0.5) / (n + 1.0);
                        let rest = (1.0 - pb - pm).max(1e-6);
                        let keyed: Vec<u8> = match keys.get(f).and_then(|kf| kf.get(q)) {
                            Some(KeyCell::Options(v)) => v.clone(),
                            _ => Vec::new(),
                        };
                        let mut lp = vec![0.0; k];
                        for (o, slot) in lp.iter_mut().enumerate().take(m) {
                            let p = if keyed.is_empty() {
                                rest / m as f64
                            } else if keyed.contains(&(o as u8)) {
                                p_bar * rest / keyed.len() as f64
                            } else {
                                (1.0 - p_bar) / (m - keyed.len()).max(1) as f64 * rest
                            };
                            *slot = p.max(1e-9).ln();
                        }
                        lp[m] = pb.ln();
                        lp[m + 1] = pm.ln();
                        lp
                    })
                    .collect(),
            )
        })
        .collect()
}

/// Infers forms; returns per booklet the inference evidence (booklets without usable
/// responses get `None`). A booklet is scored against an empirical model of its own declared
/// form **leave-one-out**, so its own responses never vouch for its declaration.
pub fn infer_forms(
    booklets: &[Booklet],
    keys: &[Vec<KeyCell>],
    questions: usize,
    options: u8,
) -> Vec<Option<FormInference>> {
    if keys.len() < 2 {
        return vec![None; booklets.len()];
    }
    let m = usize::from(options);
    let models = model(booklets, keys, questions, options);
    booklets
        .iter()
        .map(|b| {
            let ll: Vec<f64> = models
                .iter()
                .enumerate()
                .map(|(f, fm)| {
                    let own = b.declared_form == Some(f);
                    (0..questions)
                        .filter_map(|q| {
                            let c = b.questions.get(q).and_then(|s| category(&s.current, m))?;
                            Some(match fm {
                                FormModel::Fallback(lp) => lp[q][c],
                                FormModel::Empirical { counts, n } => {
                                    let (mut x, mut nq) = (counts[q][c], n[q]);
                                    if own
                                        && let Some(e) = evidence(b, q).and_then(|r| category(r, m))
                                    {
                                        nq = nq.saturating_sub(1);
                                        if e == c {
                                            x = x.saturating_sub(1);
                                        }
                                    }
                                    let den = f64::from(nq) + 0.5 * (m as f64 + 2.0);
                                    ((f64::from(x) + 0.5) / den).ln()
                                }
                            })
                        })
                        .sum()
                })
                .collect();
            let used = (0..questions)
                .filter(|&q| {
                    b.questions
                        .get(q)
                        .and_then(|s| category(&s.current, m))
                        .is_some()
                })
                .count();
            if used == 0 {
                return None;
            }
            let mut order: Vec<usize> = (0..ll.len()).collect();
            order.sort_by(|&a, &c| ll[c].total_cmp(&ll[a]).then(a.cmp(&c)));
            let argmax = order[0];
            let margin = ll[order[0]] - ll[order[1]];
            Some(FormInference {
                ll_milli: ll.iter().map(|x| (x * 1000.0).round() as i64).collect(),
                argmax,
                margin_milli: (margin * 1000.0).round() as i64,
            })
        })
        .collect()
}

/// IDN-003 (ERROR, Booklet): declared form ≠ LL-argmax by ≥ 10 nats.
pub fn idn003(serial: u32, declared: &str, argmax: &str, delta: f64) -> Exception {
    Exception::must("IDN-003", Severity::Error, Scope::Booklet)
        .with_target(Target {
            booklet: Some(serial),
            ..Target::default()
        })
        .with_detail(format!(
            "declared form {declared} but the responses fit form {argmax} better by {delta:.1} nats; \
             never switched automatically"
        ))
}

/// SCO-005 `inferred` (ERROR, Booklet): no declared form; provisional argmax.
pub fn sco005_inferred(serial: u32, argmax: &str, delta: f64) -> Exception {
    Exception::must("SCO-005", Severity::Error, Scope::Booklet)
        .with_sub_reason("inferred")
        .with_target(Target {
            booklet: Some(serial),
            ..Target::default()
        })
        .with_detail(format!(
            "no form declared; provisional form {argmax} by key log-likelihood (margin {delta:.1} nats{})",
            if delta >= DELTA_LL_NATS {
                ", confirmable as a group"
            } else {
                ""
            }
        ))
}
