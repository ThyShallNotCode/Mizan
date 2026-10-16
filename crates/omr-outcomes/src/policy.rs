//! Attainment policy objects (spec §11.4): parameters, defaults, the shipped practice
//! templates, the institution → course cascade and versioning.

use omr_model::{Exception, Scope, Severity};
use omr_scoring::{Exact, exact};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};
use std::collections::BTreeMap;

/// How P_{n,c} weights assessments (§11.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum StudentMethod {
    /// ω = W_a·Σ w·M_i / M_a: evidence weighted as the grade is (default).
    #[default]
    GradeConsistent,
    /// ω = Σ w·M_i: ignores W_a.
    PointsPooled,
    /// ω = 1 per assessment with evidence.
    AssessmentEqual,
    /// ω = V_{a,c}: (assessment ID, CLO) → weight. Serialized as a list of
    /// `[[assessment, clo], weight]` pairs, because JSON object keys must be strings.
    Custom(#[serde(with = "pair_keyed_map")] BTreeMap<(String, String), Exact>),
}

/// Serde for maps keyed by `(String, String)`: a sorted sequence of (key, value) pairs, so
/// the policy serializes to JSON (and hashes) instead of failing on a non-string key.
mod pair_keyed_map {
    use omr_scoring::Exact;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::collections::BTreeMap;

    type Map = BTreeMap<(String, String), Exact>;

    pub fn serialize<S: Serializer>(m: &Map, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(m.iter())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Map, D::Error> {
        let pairs: Vec<((String, String), Exact)> = Vec::deserialize(d)?;
        Ok(pairs.into_iter().collect())
    }
}

/// Class attainment statistic (§11.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ClassMethod {
    /// μ_c ≥ T_c ("vertical").
    Average,
    /// π_c = #{P ≥ τ}/N ≥ T_c ("horizontal"; default until Q-28 sets one).
    #[default]
    ThresholdProportion,
    /// Taif ASPIRE per-question ρ_c ≥ T_c.
    PerQuestion,
    /// L_c = max{ℓ : π_c ≥ T_ℓ} ≥ target level.
    Levels,
    /// FCAR/EAMU vectors with red/yellow/green flags.
    Eamu,
}

impl ClassMethod {
    /// Scale family for blends and roll-ups (SCO-035): proportions, levels, or EAMU.
    pub fn scale(self) -> Scale {
        match self {
            ClassMethod::Average | ClassMethod::ThresholdProportion | ClassMethod::PerQuestion => {
                Scale::Proportion
            }
            ClassMethod::Levels => Scale::Levels,
            ClassMethod::Eamu => Scale::Eamu,
        }
    }

    /// Footnote name.
    pub fn name(self) -> &'static str {
        match self {
            ClassMethod::Average => "average",
            ClassMethod::ThresholdProportion => "threshold-proportion",
            ClassMethod::PerQuestion => "per-question",
            ClassMethod::Levels => "levels",
            ClassMethod::Eamu => "EAMU",
        }
    }
}

/// Scale family (§11.3 "Like scales only").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scale {
    /// A proportion in [0, 1].
    Proportion,
    /// An attainment level 0–3.
    Levels,
    /// EAMU: never blended or rolled up.
    Eamu,
}

/// TP-154 "Actual level" display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ActualLevelDisplay {
    /// Average and threshold-proportion together, with a footnote (default).
    #[default]
    Both,
    /// Only the deciding method.
    MethodOnly,
}

/// Verdict basis (§11.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VerdictBasis {
    /// The point estimate (default).
    #[default]
    Point,
    /// The lower CI bound.
    CiLower,
}

/// EAMU population.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum EamuPopulation {
    /// All N_c students (default).
    #[default]
    All,
    /// Students with a passing letter.
    Passing,
}

/// Bonus items as CLO evidence (§11.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BonusEvidence {
    /// Excluded.
    Never,
    /// In P_{n,c,a} only for students with a non-blank response (default).
    #[default]
    AttemptedOnly,
    /// Treated as ordinary items.
    All,
}

/// Section aggregation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SectionAggregation {
    /// Pooled students (default).
    #[default]
    Pooled,
    /// Mean of section rates (opt-in; Simpson's-paradox warning).
    MeanOfSections,
}

/// PLO roll-up mode (§11.3, §11.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum PloRollup {
    /// Strength-weighted over course CLO attainments (default).
    #[default]
    Course,
    /// Student-level P_{n,p}.
    Student,
    /// Credit-hour weighted AR_p (UoM).
    CreditHour,
}

/// FCAR flag thresholds (red avg₅, red U share, green avg₅).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FcarFlags {
    /// Red when avg₅ is below this…
    pub red_avg5: Exact,
    /// …and the U share exceeds this.
    pub red_u_share: Exact,
    /// Green when avg₅ exceeds this and U = 0.
    pub green_avg5: Exact,
}

/// Thin-evidence thresholds (items, course share, assessments).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThinEvidence {
    /// Fewer items than this is thin.
    pub items: usize,
    /// Σ e_{i,c} below this share of the course is thin.
    pub course_share: Exact,
    /// Fewer assessments than this is thin.
    pub assessments: usize,
}

/// Report groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReportGroups {
    /// Per section.
    pub section: bool,
    /// Per campus/branch.
    pub campus: bool,
    /// Per track (male/female).
    pub track: bool,
}

/// An attainment policy version (§11.4). JSON, SHA-384-identified, with `effective_from_term`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttainmentPolicy {
    /// Template or policy name.
    pub name: String,
    /// Version.
    pub version: u32,
    /// First term the version applies to.
    pub effective_from_term: String,
    /// Student method.
    pub student_method: StudentMethod,
    /// Deciding class method.
    pub class_method: ClassMethod,
    /// TP-154 display.
    pub actual_level_display: ActualLevelDisplay,
    /// τ (student threshold), default 0.60.
    pub tau: Exact,
    /// T (class target), default 0.70.
    pub target: Exact,
    /// Per-CLO (τ, T) overrides.
    pub clo_overrides: BTreeMap<String, (Option<Exact>, Option<Exact>)>,
    /// Level cut-points (T₁, T₂, T₃).
    pub levels: [Exact; 3],
    /// Target level for the levels method.
    pub target_level: u8,
    /// EAMU cut-points (E, A, M).
    pub eamu_bands: [Exact; 3],
    /// EAMU population.
    pub eamu_population: EamuPopulation,
    /// FCAR flags.
    pub fcar_flags: FcarFlags,
    /// Verdict basis.
    pub verdict_basis: VerdictBasis,
    /// β for direct/indirect blends.
    pub beta: Exact,
    /// Blending allowed (PSU: false locks β = 1).
    pub blend_allowed: bool,
    /// Likert cut-off for indirect evidence (default ≥ 4 of 5).
    pub indirect_cutoff: u8,
    /// Section aggregation.
    pub section_aggregation: SectionAggregation,
    /// Report groups.
    pub report_groups: ReportGroups,
    /// Per-student coverage floor (share of planned Σ ω).
    pub min_evidence_share: Exact,
    /// Thin-evidence thresholds.
    pub thin_evidence: ThinEvidence,
    /// N_c below this raises SCO-023 `clo_small_n`.
    pub small_n: usize,
    /// Shared outputs suppress cells with n below this.
    pub suppress_n: usize,
    /// Bonus items as evidence.
    pub bonus_items_as_evidence: BonusEvidence,
    /// L5 drop-lowest also removes CLO evidence.
    pub clo_respects_drop: bool,
    /// PLO roll-up.
    pub plo_rollup: PloRollup,
    /// Use only M-strength CLO links in PLO roll-ups.
    pub m_only: bool,
    /// γ, the exit-exam weight.
    pub exit_exam_weight: Exact,
    /// Bootstrap replicates for small-N CIs (§11.3: B = 2,000).
    pub bootstrap_b: usize,
}

impl Default for AttainmentPolicy {
    /// The §11.4 defaults.
    fn default() -> Self {
        Self {
            name: "default".into(),
            version: 1,
            effective_from_term: String::new(),
            student_method: StudentMethod::GradeConsistent,
            class_method: ClassMethod::ThresholdProportion,
            actual_level_display: ActualLevelDisplay::Both,
            tau: exact(6, 10),
            target: exact(7, 10),
            clo_overrides: BTreeMap::new(),
            levels: [exact(6, 10), exact(7, 10), exact(8, 10)],
            target_level: 2,
            eamu_bands: [exact(9, 10), exact(75, 100), exact(6, 10)],
            eamu_population: EamuPopulation::All,
            fcar_flags: FcarFlags {
                red_avg5: exact(33, 10),
                red_u_share: exact(1, 10),
                green_avg5: exact(46, 10),
            },
            verdict_basis: VerdictBasis::Point,
            beta: exact(1, 1),
            blend_allowed: true,
            indirect_cutoff: 4,
            section_aggregation: SectionAggregation::Pooled,
            report_groups: ReportGroups {
                section: true,
                campus: true,
                track: true,
            },
            min_evidence_share: exact(1, 2),
            thin_evidence: ThinEvidence {
                items: 3,
                course_share: exact(1, 10),
                assessments: 2,
            },
            small_n: 10,
            suppress_n: 5,
            bonus_items_as_evidence: BonusEvidence::AttemptedOnly,
            clo_respects_drop: false,
            plo_rollup: PloRollup::Course,
            m_only: false,
            exit_exam_weight: exact(0, 1),
            bootstrap_b: 2000,
        }
    }
}

impl AttainmentPolicy {
    /// τ_c for a CLO.
    pub fn tau_for(&self, clo: &str) -> Exact {
        self.clo_overrides
            .get(clo)
            .and_then(|o| o.0)
            .unwrap_or(self.tau)
    }

    /// T_c for a CLO.
    pub fn target_for(&self, clo: &str) -> Exact {
        self.clo_overrides
            .get(clo)
            .and_then(|o| o.1)
            .unwrap_or(self.target)
    }

    /// SHA-384 (hex) of the canonical JSON: the `attainment_policy_hash` of §11.4.
    ///
    /// Every field serializes to JSON; should that ever fail, the deterministic `Debug`
    /// rendering is hashed instead, so two different policies never share the hash of an
    /// empty input.
    pub fn hash(&self) -> String {
        let bytes = serde_json::to_vec(self)
            .unwrap_or_else(|e| format!("unserializable policy ({e}): {self:?}").into_bytes());
        Sha384::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// SCO-035 (ERROR): a blend with indirect evidence on a scale that cannot blend (EAMU
    /// is never blended; levels blend only with levels, §11.3), or β outside [0, 1].
    pub fn validate(&self, has_indirect: bool) -> Vec<Exception> {
        let mut ex = Vec::new();
        let sco035 = |d: &str| {
            Exception::must("SCO-035", Severity::Error, Scope::OutcomeReport).with_detail(d)
        };
        let one = exact(1, 1);
        if self.beta < exact(0, 1) || self.beta > one {
            ex.push(sco035("β must lie in [0, 1]"));
        }
        if has_indirect && self.blend_allowed && self.beta < one {
            match self.class_method.scale() {
                Scale::Eamu => ex.push(sco035(
                    "EAMU results are never blended with indirect evidence",
                )),
                Scale::Levels => ex.push(sco035(
                    "levels blend only with levels; indirect evidence here is a proportion",
                )),
                Scale::Proportion => {}
            }
        }
        ex
    }

    /// SEU practice: threshold-proportion, τ = 0.70, T = 0.75, opt-in 70/30 blend.
    pub fn seu() -> Self {
        Self {
            name: "SEU practice".into(),
            class_method: ClassMethod::ThresholdProportion,
            tau: exact(7, 10),
            target: exact(75, 100),
            beta: exact(7, 10),
            ..Self::default()
        }
    }

    /// UoM practice: average and threshold-proportion together, T = 0.70, credit-hour
    /// roll-up with γ = 0.3.
    pub fn uom() -> Self {
        Self {
            name: "UoM practice".into(),
            class_method: ClassMethod::Average,
            actual_level_display: ActualLevelDisplay::Both,
            target: exact(7, 10),
            plo_rollup: PloRollup::CreditHour,
            exit_exam_weight: exact(3, 10),
            ..Self::default()
        }
    }

    /// Taif ASPIRE practice: per-question, 60 % target.
    pub fn taif() -> Self {
        Self {
            name: "Taif ASPIRE practice".into(),
            class_method: ClassMethod::PerQuestion,
            target: exact(6, 10),
            ..Self::default()
        }
    }

    /// PSU practice: no blending of direct and indirect evidence, T = 0.75 (the rubric
    /// method itself is deferred in the MVP; threshold-proportion decides).
    pub fn psu() -> Self {
        Self {
            name: "PSU practice".into(),
            target: exact(75, 100),
            blend_allowed: false,
            ..Self::default()
        }
    }

    /// ABET FCAR/EAMU (IU EvalTools practice): EAMU over passing students with flags.
    pub fn abet_fcar() -> Self {
        Self {
            name: "ABET FCAR/EAMU".into(),
            class_method: ClassMethod::Eamu,
            eamu_population: EamuPopulation::Passing,
            ..Self::default()
        }
    }

    /// ABET generic: threshold-proportion; the programme sets τ and T.
    pub fn abet_generic(tau: Exact, target: Exact) -> Self {
        Self {
            name: "ABET generic".into(),
            tau,
            target,
            ..Self::default()
        }
    }
}

/// A partial policy for the institution → college → programme → course cascade (§11.4):
/// a more specific level overrides only the parameters it sets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyPatch {
    /// Class method.
    pub class_method: Option<ClassMethod>,
    /// Student method.
    pub student_method: Option<StudentMethod>,
    /// τ.
    pub tau: Option<Exact>,
    /// T.
    pub target: Option<Exact>,
    /// β.
    pub beta: Option<Exact>,
    /// Blend allowed.
    pub blend_allowed: Option<bool>,
    /// EAMU population.
    pub eamu_population: Option<EamuPopulation>,
    /// Verdict basis.
    pub verdict_basis: Option<VerdictBasis>,
    /// PLO roll-up.
    pub plo_rollup: Option<PloRollup>,
    /// γ.
    pub exit_exam_weight: Option<Exact>,
}

impl PolicyPatch {
    /// Applies the patch on top of `base`.
    pub fn apply(&self, base: &AttainmentPolicy) -> AttainmentPolicy {
        let mut p = base.clone();
        if let Some(v) = self.class_method {
            p.class_method = v;
        }
        if let Some(v) = &self.student_method {
            p.student_method = v.clone();
        }
        if let Some(v) = self.tau {
            p.tau = v;
        }
        if let Some(v) = self.target {
            p.target = v;
        }
        if let Some(v) = self.beta {
            p.beta = v;
        }
        if let Some(v) = self.blend_allowed {
            p.blend_allowed = v;
        }
        if let Some(v) = self.eamu_population {
            p.eamu_population = v;
        }
        if let Some(v) = self.verdict_basis {
            p.verdict_basis = v;
        }
        if let Some(v) = self.plo_rollup {
            p.plo_rollup = v;
        }
        if let Some(v) = self.exit_exam_weight {
            p.exit_exam_weight = v;
        }
        p
    }
}

/// Resolves the cascade institution → college → programme → course.
pub fn cascade(institution: &AttainmentPolicy, patches: &[&PolicyPatch]) -> AttainmentPolicy {
    patches
        .iter()
        .fold(institution.clone(), |p, patch| patch.apply(&p))
}

/// SCO-025 (INFO) when a new policy version takes effect within the term of the old one;
/// results bind to the version in force, history is kept.
pub fn mid_term_change(
    old: &AttainmentPolicy,
    new: &AttainmentPolicy,
    term: &str,
) -> Option<Exception> {
    (old.hash() != new.hash() && new.effective_from_term == term && old.effective_from_term == term)
        .then(|| {
            Exception::must("SCO-025", Severity::Info, Scope::OutcomeReport).with_detail(format!(
                "attainment policy changed mid-term ({term}): v{} → v{}; recomputed, history kept",
                old.version, new.version
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_overrides_hash_and_cascade() {
        let p = AttainmentPolicy::default();
        assert_eq!((p.tau, p.target), (exact(6, 10), exact(7, 10)));
        assert_eq!(p.class_method, ClassMethod::ThresholdProportion);
        let mut q = p.clone();
        q.clo_overrides
            .insert("CLO2".into(), (None, Some(exact(8, 10))));
        assert_eq!(q.target_for("CLO2"), exact(8, 10));
        assert_eq!(q.tau_for("CLO2"), exact(6, 10));
        assert_ne!(p.hash(), q.hash());
        assert_eq!(
            p.hash(),
            AttainmentPolicy::default().hash(),
            "hash is deterministic"
        );
        // Institution sets τ; the course overrides only T.
        let inst = PolicyPatch {
            tau: Some(exact(7, 10)),
            ..PolicyPatch::default()
        };
        let course = PolicyPatch {
            target: Some(exact(75, 100)),
            ..PolicyPatch::default()
        };
        let r = cascade(&AttainmentPolicy::default(), &[&inst, &course]);
        assert_eq!((r.tau, r.target), (exact(7, 10), exact(75, 100)));
        assert!(AttainmentPolicy::seu().validate(true).is_empty());
    }

    #[test]
    fn custom_method_serializes_and_hashes_by_content() {
        // V_{a,c} is keyed by (assessment, CLO): JSON needs string keys, so the map must
        // serialize as pairs. Two custom policies that differ only in V must hash apart.
        let custom = |v: Exact| AttainmentPolicy {
            student_method: StudentMethod::Custom(BTreeMap::from([(
                ("final".to_string(), "CLO2".to_string()),
                v,
            )])),
            ..AttainmentPolicy::default()
        };
        let (a, b) = (custom(exact(6, 10)), custom(exact(9, 10)));
        let json = serde_json::to_string(&a).expect("custom policy serializes");
        let back: AttainmentPolicy = serde_json::from_str(&json).expect("and round-trips");
        assert_eq!(back, a);
        assert_ne!(a.hash(), b.hash());
        assert_eq!(a.hash(), custom(exact(6, 10)).hash());
        // SHA-384 of the empty input: what every custom policy hashed to before.
        assert_ne!(
            a.hash(),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
        );
    }
}
