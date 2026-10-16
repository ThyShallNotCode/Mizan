//! Outcome frameworks, PLOs, CLO sets and the CLO → PLO → framework-outcome crosswalk
//! (spec §11.1, §11.2 "CloPloMap").

use omr_scoring::Exact;
use serde::{Deserialize, Serialize};

/// The three top-level NQF-KSA domains that course reports group CLOs by (§11.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Domain {
    /// Knowledge and Understanding (PLO codes K1, K2, …).
    Knowledge,
    /// Skills (cognitive; practical and physical; communication, ICT and numeracy): S1, …
    Skills,
    /// Values, Autonomy and Responsibility: V1, …
    Values,
}

impl Domain {
    /// All domains in TP-154 order.
    pub const ALL: [Domain; 3] = [Domain::Knowledge, Domain::Skills, Domain::Values];

    /// Code prefix: `K`, `S` or `V`.
    pub fn prefix(self) -> char {
        match self {
            Domain::Knowledge => 'K',
            Domain::Skills => 'S',
            Domain::Values => 'V',
        }
    }

    /// The domain of a PLO code such as `K1` or `S3`.
    pub fn of_code(code: &str) -> Option<Domain> {
        match code.trim().chars().next()?.to_ascii_uppercase() {
            'K' => Some(Domain::Knowledge),
            'S' => Some(Domain::Skills),
            'V' => Some(Domain::Values),
            _ => None,
        }
    }

    /// TP-154 row-group number (1.0, 2.0, 3.0).
    pub fn number(self) -> u8 {
        match self {
            Domain::Knowledge => 1,
            Domain::Skills => 2,
            Domain::Values => 3,
        }
    }

    /// English heading.
    pub fn name_en(self) -> &'static str {
        match self {
            Domain::Knowledge => "Knowledge and understanding",
            Domain::Skills => "Skills",
            Domain::Values => "Values, autonomy, and responsibility",
        }
    }

    /// Arabic heading.
    pub fn name_ar(self) -> &'static str {
        match self {
            Domain::Knowledge => "المعرفة والفهم",
            Domain::Skills => "المهارات",
            Domain::Values => "القيم والاستقلالية والمسؤولية",
        }
    }
}

/// A programme learning outcome (§11.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Plo {
    /// Code, e.g. `K1`.
    pub code: String,
    /// NQF domain.
    pub domain: Domain,
    /// English text.
    pub text_en: String,
    /// Arabic text.
    pub text_ar: String,
    /// Version.
    pub version: u32,
}

/// One outcome of an accreditor framework (ABET SO, NQF domain, custom).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameworkOutcome {
    /// Code, e.g. `SO1`.
    pub code: String,
    /// Short label (official wording comes from the versioned framework package).
    pub text: String,
    /// NQF domain where the framework assigns one.
    pub domain: Option<Domain>,
    /// Version of the outcome text.
    pub version: u32,
    /// A framework-specific extra outcome (e.g. IAU's "SO 0" = K1), projected into this
    /// framework only.
    pub extra: bool,
}

/// An outcome framework template (§11.1 table).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutcomeFramework {
    /// Stable identifier, e.g. `abet-eac`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Edition, e.g. `2026-27`.
    pub edition: String,
    /// Outcomes in order.
    pub outcomes: Vec<FrameworkOutcome>,
}

fn fo(code: &str, text: &str, domain: Option<Domain>) -> FrameworkOutcome {
    FrameworkOutcome {
        code: code.into(),
        text: text.into(),
        domain,
        version: 1,
        extra: false,
    }
}

impl OutcomeFramework {
    /// NQF-KSA 2nd ed. (1444/2023): the three top-level domains course reports group by.
    pub fn nqf_ksa() -> Self {
        Self {
            id: "nqf-ksa".into(),
            name: "National Qualifications Framework (NQF-KSA)".into(),
            edition: "2nd ed. 1444/2023".into(),
            outcomes: Domain::ALL
                .iter()
                .map(|d| fo(&d.prefix().to_string(), d.name_en(), Some(*d)))
                .collect(),
        }
    }

    /// ABET EAC Criterion 3 student outcomes 1–7 (2026–27). Labels are summaries; official
    /// wording is versioned in the framework package. No numeric thresholds are prescribed.
    pub fn abet_eac_2026() -> Self {
        let s = Some(Domain::Skills);
        let v = Some(Domain::Values);
        Self {
            id: "abet-eac".into(),
            name: "ABET EAC Criterion 3".into(),
            edition: "2026-27".into(),
            outcomes: vec![
                fo(
                    "SO1",
                    "Identify, formulate and solve complex engineering problems",
                    s,
                ),
                fo("SO2", "Apply engineering design to meet specified needs", s),
                fo(
                    "SO3",
                    "Communicate effectively with a range of audiences",
                    s,
                ),
                fo(
                    "SO4",
                    "Ethical and professional responsibilities; informed judgments",
                    v,
                ),
                fo("SO5", "Function effectively on a team", v),
                fo(
                    "SO6",
                    "Develop and conduct experimentation; analyze and interpret data",
                    s,
                ),
                fo(
                    "SO7",
                    "Acquire and apply new knowledge (learning strategies)",
                    v,
                ),
            ],
        }
    }

    /// ABET CAC student outcomes 1–5 plus the Program Criteria SO 6 (e.g. CS), whose text
    /// is versioned per Program Criteria.
    pub fn abet_cac_2026(so6: &str) -> Self {
        let s = Some(Domain::Skills);
        let v = Some(Domain::Values);
        Self {
            id: "abet-cac".into(),
            name: "ABET CAC Criterion 3".into(),
            edition: "2026-27".into(),
            outcomes: vec![
                fo(
                    "SO1",
                    "Analyze complex computing problems and identify solutions",
                    s,
                ),
                fo(
                    "SO2",
                    "Design, implement and evaluate computing-based solutions",
                    s,
                ),
                fo("SO3", "Communicate effectively in professional contexts", s),
                fo(
                    "SO4",
                    "Professional responsibilities and informed judgments",
                    v,
                ),
                fo("SO5", "Function effectively as a team member or leader", v),
                fo("SO6", so6, s),
            ],
        }
    }

    /// Adds a framework-specific extra outcome (e.g. "SO0" = K1 at IAU).
    pub fn with_extra(mut self, code: &str, text: &str, domain: Option<Domain>) -> Self {
        let mut o = fo(code, text, domain);
        o.extra = true;
        self.outcomes.insert(0, o);
        self
    }
}

/// A course learning outcome (§11.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Clo {
    /// Code, e.g. `CLO1` or `1.1`.
    pub code: String,
    /// English text.
    pub text_en: String,
    /// Arabic text.
    pub text_ar: String,
    /// NQF domain.
    pub domain: Domain,
    /// Revised-Bloom level 1–6.
    pub bloom: Option<u8>,
    /// τ_c override (student threshold).
    pub tau: Option<Exact>,
    /// T_c override (class target; prints as TP-154 "Targeted level").
    pub target: Option<Exact>,
    /// Version of this CLO.
    pub version: u32,
    /// Retired versions may not receive new mappings (SCO-011 `retired_clo_version`).
    pub retired: bool,
}

impl Clo {
    /// A current CLO with no overrides.
    pub fn new(code: &str, text_en: &str, domain: Domain) -> Self {
        Self {
            code: code.into(),
            text_en: text_en.into(),
            text_ar: String::new(),
            domain,
            bloom: None,
            tau: None,
            target: None,
            version: 1,
            retired: false,
        }
    }
}

/// A versioned CLO set of one course (§11.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CloSet {
    /// Course code.
    pub course: String,
    /// Version (from the TP-153 cover).
    pub version: u32,
    /// CLOs.
    pub clos: Vec<Clo>,
}

impl CloSet {
    /// The CLO with `code`.
    pub fn clo(&self, code: &str) -> Option<&Clo> {
        self.clos.iter().find(|c| c.code == code)
    }
}

/// CLO → PLO link with strength m ∈ {1, 2, 3} = I/P/M (§11.2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CloPloMap {
    /// CLO code.
    pub clo: String,
    /// PLO code.
    pub plo: String,
    /// Strength 1 = Introduced, 2 = Practiced, 3 = Mastered.
    pub strength: u8,
    /// Notes.
    pub notes: String,
    /// Approver.
    pub approved_by: Option<String>,
    /// Version.
    pub version: u32,
}

impl CloPloMap {
    /// An approved link.
    pub fn new(clo: &str, plo: &str, strength: u8) -> Self {
        Self {
            clo: clo.into(),
            plo: plo.into(),
            strength,
            notes: String::new(),
            approved_by: None,
            version: 1,
        }
    }

    /// I/P/M letter of the strength.
    pub fn ipm(&self) -> char {
        match self.strength {
            1 => 'I',
            2 => 'P',
            _ => 'M',
        }
    }
}

/// PLO → framework outcome link with strength x (same shape as [`CloPloMap`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Crosswalk {
    /// PLO code.
    pub plo: String,
    /// Framework outcome code (e.g. `SO1`).
    pub outcome: String,
    /// Strength x.
    pub strength: u8,
    /// Notes.
    pub notes: String,
    /// Approver.
    pub approved_by: Option<String>,
}

impl Crosswalk {
    /// A link.
    pub fn new(plo: &str, outcome: &str, strength: u8) -> Self {
        Self {
            plo: plo.into(),
            outcome: outcome.into(),
            strength,
            notes: String::new(),
            approved_by: None,
        }
    }
}

/// Course catalogue entry (TP-153 section A credit hours).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Course {
    /// Code, e.g. `CS 211`.
    pub code: String,
    /// Title.
    pub title: String,
    /// Credit hours C_k.
    pub credit_hours: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_codes_and_numbers() {
        assert_eq!(Domain::of_code("S3"), Some(Domain::Skills));
        assert_eq!(Domain::of_code("X1"), None);
        assert_eq!(Domain::Values.number(), 3);
        assert_eq!(Domain::Knowledge.prefix(), 'K');
        assert_eq!(CloPloMap::new("CLO1", "K1", 2).ipm(), 'P');
    }
}
