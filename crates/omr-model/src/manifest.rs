//! Engine manifest and the key-blind batch context (spec §14.6, §4.3 rule R2).

use crate::ids::{ExamId, TemplateRevision};
use serde::{Deserialize, Serialize};

/// Records what produced a recognition run so it can be reproduced (§14.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EngineManifest {
    /// Application version (`CARGO_PKG_VERSION` of the app).
    pub app_version: String,
    /// Source revision, if known.
    pub git_sha: Option<String>,
    /// SHA-384 (hex) of the canonical template revision.
    pub template_sha384: String,
    /// SHA-384 (hex) of the recognition configuration and thresholds.
    pub config_sha384: String,
    /// SHA-384 (hex) of the calibration (thresholds fitted on the calibration split), if any.
    pub calibration_sha384: Option<String>,
    /// CPU feature level used for SIMD dispatch (decisions must not depend on it, §23.6).
    pub cpu_level: String,
}

/// What recognition receives for a batch (R2): template revision, manifest, batch statistics.
/// It deliberately has **no key, policy or answer fields**.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatchContext {
    /// The exam being recognised.
    pub exam: ExamId,
    /// Template revision expected for this batch.
    pub revision: TemplateRevision,
    /// Manifest stamped on every result.
    pub manifest: EngineManifest,
    /// High-stakes mode raises the auto-accept thresholds (§8.8).
    pub high_stakes: bool,
}
