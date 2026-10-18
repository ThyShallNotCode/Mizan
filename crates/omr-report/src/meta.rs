//! Report metadata: classification label, language and the provenance footer (spec §16.3,
//! §17.1, §11.7).

use serde::{Deserialize, Serialize};

/// The DMO-configured data classification (NDMO levels). Every export carries the highest
/// label of its contents in its header and footer, file metadata and manifest (§16.3).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Classification {
    /// Public (عام).
    Public,
    /// Restricted (مقيد): the default for grades, scans, rosters and outcomes.
    #[default]
    Restricted,
    /// Secret (سري).
    Secret,
    /// Top secret (سري للغاية).
    TopSecret,
}

impl Classification {
    /// English label.
    pub fn en(self) -> &'static str {
        match self {
            Classification::Public => "Public",
            Classification::Restricted => "Restricted",
            Classification::Secret => "Secret",
            Classification::TopSecret => "Top Secret",
        }
    }

    /// Arabic label.
    pub fn ar(self) -> &'static str {
        match self {
            Classification::Public => "عام",
            Classification::Restricted => "مقيد",
            Classification::Secret => "سري",
            Classification::TopSecret => "سري للغاية",
        }
    }

    /// Bilingual label, e.g. `Restricted / مقيد`.
    pub fn label(self) -> String {
        format!("{} / {}", self.en(), self.ar())
    }

    /// The higher of two labels (an export takes the highest of its contents).
    pub fn max(self, other: Self) -> Self {
        Ord::max(self, other)
    }
}

/// Report language (the programme's language, §20.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    /// English.
    #[default]
    En,
    /// Arabic (RTL sheets and pages).
    Ar,
}

impl Language {
    /// BCP 47 tag.
    pub fn tag(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Ar => "ar-SA",
        }
    }

    /// True for right-to-left.
    pub fn rtl(self) -> bool {
        self == Language::Ar
    }
}

/// Header and provenance of one report (§17.1 "Footer"): the document date is the snapshot
/// time, never "now", so output is deterministic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportMeta {
    /// Title (English).
    pub title: String,
    /// Title (Arabic), if any.
    pub title_ar: Option<String>,
    /// Institution.
    pub institution: Option<String>,
    /// Course code.
    pub course_code: Option<String>,
    /// Course name.
    pub course_name: Option<String>,
    /// Course name in Arabic.
    pub course_name_ar: Option<String>,
    /// Term label as the university writes it (Hijri and Gregorian).
    pub term: Option<String>,
    /// Section(s) or campus scope.
    pub scope: Option<String>,
    /// Classification label.
    pub classification: Classification,
    /// Language.
    pub language: Language,
    /// Snapshot time (UTC, RFC 3339): the document date.
    pub snapshot_utc: String,
    /// Score run ID (§10.1).
    pub score_run_id: Option<String>,
    /// Key version.
    pub key_version: Option<u32>,
    /// Policy version.
    pub policy_version: Option<u32>,
    /// Roster snapshot hash.
    pub roster_snapshot: Option<String>,
    /// Engine manifest hash.
    pub engine_manifest: Option<String>,
    /// Report template hash.
    pub template_hash: Option<String>,
    /// Audit-chain head (§14.4).
    pub chain_head: Option<String>,
    /// AI label: "no AI" or the AI-generated sections (§18.5).
    pub ai_label: String,
    /// "Draft mappings" watermark (SCO-032 open).
    pub draft: bool,
    /// "SUPERSEDED by run X, date" watermark (§17.1).
    pub superseded_by: Option<String>,
}

impl ReportMeta {
    /// Minimal metadata: Restricted, English, "no AI".
    pub fn new(title: &str, snapshot_utc: &str) -> Self {
        Self {
            title: title.into(),
            title_ar: None,
            institution: None,
            course_code: None,
            course_name: None,
            course_name_ar: None,
            term: None,
            scope: None,
            classification: Classification::Restricted,
            language: Language::En,
            snapshot_utc: snapshot_utc.into(),
            score_run_id: None,
            key_version: None,
            policy_version: None,
            roster_snapshot: None,
            engine_manifest: None,
            template_hash: None,
            chain_head: None,
            ai_label: "no AI".into(),
            draft: false,
            superseded_by: None,
        }
    }

    /// The provenance lines of the footer, in a fixed order (absent values are omitted).
    pub fn provenance(&self) -> Vec<(&'static str, String)> {
        let short = |h: &str| h.chars().take(16).collect::<String>();
        let mut v = vec![("Classification", self.classification.label())];
        if let Some(x) = &self.score_run_id {
            v.push(("Score run", short(x)));
        }
        if let Some(k) = self.key_version {
            v.push(("Key version", k.to_string()));
        }
        if let Some(k) = self.policy_version {
            v.push(("Policy version", k.to_string()));
        }
        if let Some(x) = &self.roster_snapshot {
            v.push(("Roster snapshot", short(x)));
        }
        if let Some(x) = &self.engine_manifest {
            v.push(("Engine manifest", short(x)));
        }
        if let Some(x) = &self.template_hash {
            v.push(("Template", short(x)));
        }
        if let Some(x) = &self.chain_head {
            v.push(("Chain head", short(x)));
        }
        v.push(("Snapshot (UTC)", self.snapshot_utc.clone()));
        v.push(("AI", self.ai_label.clone()));
        v
    }

    /// A compact provenance line for space-limited places (XLSX page footers, 255 characters):
    /// `Run 9f2c… · Key 1 · Policy 1 · Roster 5ad1… · Engine e1e2… · Chain c4a1… · <UTC> · no AI`.
    pub fn compact_footer(&self) -> String {
        let short = |h: &str, n: usize| h.chars().take(n).collect::<String>();
        let mut v = Vec::new();
        if let Some(x) = &self.score_run_id {
            v.push(format!("Run {}", short(x, 16)));
        }
        if let Some(k) = self.key_version {
            v.push(format!("Key {k}"));
        }
        if let Some(k) = self.policy_version {
            v.push(format!("Policy {k}"));
        }
        if let Some(x) = &self.roster_snapshot {
            v.push(format!("Roster {}", short(x, 8)));
        }
        if let Some(x) = &self.engine_manifest {
            v.push(format!("Engine {}", short(x, 8)));
        }
        if let Some(x) = &self.template_hash {
            v.push(format!("Template {}", short(x, 8)));
        }
        if let Some(x) = &self.chain_head {
            v.push(format!("Chain {}", short(x, 16)));
        }
        v.push(format!("{} UTC", self.snapshot_utc));
        v.push(self.ai_label.clone());
        v.join(" · ")
    }

    /// The footer as one line: `Classification: … · Score run: … · …`.
    pub fn footer_line(&self) -> String {
        self.provenance()
            .into_iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_order() {
        assert_eq!(Classification::default().label(), "Restricted / مقيد");
        assert_eq!(
            Classification::Public.max(Classification::Restricted),
            Classification::Restricted
        );
        assert_eq!(
            Classification::TopSecret.max(Classification::Secret),
            Classification::TopSecret
        );
    }

    #[test]
    fn footer_is_deterministic() {
        let mut m = ReportMeta::new("Results", "2026-10-15T09:00:00Z");
        m.score_run_id = Some("0123456789abcdef0123".into());
        m.key_version = Some(2);
        assert_eq!(
            m.footer_line(),
            "Classification: Restricted / مقيد · Score run: 0123456789abcdef · Key version: 2 · \
             Snapshot (UTC): 2026-10-15T09:00:00Z · AI: no AI"
        );
    }
}
