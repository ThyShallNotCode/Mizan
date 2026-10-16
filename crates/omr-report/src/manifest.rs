//! The SHA-384 export manifest (spec §16.3, §17.1 "Signing"): one entry per exported file,
//! with the classification label, purpose, PII flag and provenance. CSV and JSON exports carry
//! their label here only.
//!
//! The manifest serialises canonically (members sorted by UTF-16 code units here, whatever
//! `serde_json`'s map order; every number a small integer). The caller signs these exact bytes (ECDSA P-384, `omr-store`'s checkpoint
//! signer) into `manifest.sig` and records `ExportCreated` with [`ExportManifest::sha384_hex`].

use crate::meta::ReportMeta;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};

/// Manifest format identifier.
pub const MANIFEST_FORMAT: &str = "omr-export-manifest/1";

/// One exported file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Path inside the export folder.
    pub path: String,
    /// SHA-384 (hex) of the bytes.
    pub sha384: String,
    /// Length in bytes.
    pub len: u64,
    /// Media type.
    pub media_type: String,
}

/// The export manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportManifest {
    /// [`MANIFEST_FORMAT`].
    pub format: String,
    /// Classification label (the highest of the contents).
    pub classification: String,
    /// Purpose (grades, accreditation, item analysis, …).
    pub purpose: String,
    /// Carries personal data.
    pub pii: bool,
    /// Score run.
    pub score_run_id: Option<String>,
    /// Audit-chain head at export.
    pub chain_head: Option<String>,
    /// Snapshot time (the document date).
    pub snapshot_utc: String,
    /// Files, in the order added.
    pub files: Vec<ManifestFile>,
}

/// Media type from a file extension.
pub fn media_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("csv") => "text/csv; charset=utf-8",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

impl ExportManifest {
    /// An empty manifest for an export of `meta`'s report.
    pub fn new(meta: &ReportMeta, purpose: &str, pii: bool) -> Self {
        Self {
            format: MANIFEST_FORMAT.into(),
            classification: meta.classification.label(),
            purpose: purpose.into(),
            pii,
            score_run_id: meta.score_run_id.clone(),
            chain_head: meta.chain_head.clone(),
            snapshot_utc: meta.snapshot_utc.clone(),
            files: Vec::new(),
        }
    }

    /// Adds a file by its bytes.
    pub fn add(&mut self, path: &str, bytes: &[u8]) {
        self.files.push(ManifestFile {
            path: path.into(),
            sha384: hex(&Sha384::digest(bytes)),
            len: bytes.len() as u64,
            media_type: media_type(path).into(),
        });
    }

    /// Canonical JSON bytes (RFC 8785 for this content: members sorted by UTF-16 code units,
    /// no whitespace, minimal string escapes, integers only).
    pub fn to_canonical_json(&self) -> Vec<u8> {
        // Keys are sorted here, not left to `serde_json`'s map type: a dependency enabling
        // `serde_json/preserve_order` anywhere in the build would otherwise change the bytes,
        // and with them the signed manifest hash.
        let mut out = String::new();
        if let Ok(v) = serde_json::to_value(self) {
            canonical(&v, &mut out);
        }
        out.into_bytes()
    }

    /// SHA-384 (hex) of the canonical JSON: the `manifest_sha384` of `ExportCreated`.
    pub fn sha384_hex(&self) -> String {
        hex(&Sha384::digest(self.to_canonical_json()))
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Writes `v` canonically. Scalars use `serde_json`'s encoder, whose string escapes (`\"`,
/// `\\`, `\b \f \n \r \t`, other controls as lower-case `\u00xx`, everything else literal)
/// and integer form are those of RFC 8785; the manifest holds no floats.
fn canonical(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                canonical(&m[k.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&serde_json::to_string(scalar).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_canonical_and_hashes_files() {
        let meta = ReportMeta::new("Results", "2026-10-15T09:00:00Z");
        let mut m = ExportManifest::new(&meta, "grades", true);
        m.add("results.csv", b"abc");
        let json = String::from_utf8(m.to_canonical_json()).unwrap();
        assert!(json.starts_with(
            "{\"chain_head\":null,\"classification\":\"Restricted / مقيد\",\"files\":[{\"len\":3,\
             \"media_type\":\"text/csv; charset=utf-8\",\"path\":\"results.csv\",\"sha384\":\"cb00753f"
        ));
        assert!(json.ends_with("\"snapshot_utc\":\"2026-10-15T09:00:00Z\"}"));
        assert_eq!(m.sha384_hex().len(), 96);
        assert_eq!(m.sha384_hex(), ExportManifest::sha384_hex(&m.clone()));
    }

    #[test]
    fn canonical_form_sorts_members_whatever_the_map_order() {
        // Insertion order deliberately unsorted (as under `serde_json/preserve_order`).
        let mut inner = serde_json::Map::new();
        inner.insert("z".into(), serde_json::json!(1));
        inner.insert("a\u{1}\"".into(), serde_json::json!("x\ny"));
        let mut outer = serde_json::Map::new();
        outer.insert("b".into(), serde_json::json!([true, null, 3]));
        outer.insert("a".into(), serde_json::Value::Object(inner));
        let mut out = String::new();
        canonical(&serde_json::Value::Object(outer), &mut out);
        assert_eq!(
            out,
            "{\"a\":{\"a\\u0001\\\"\":\"x\\ny\",\"z\":1},\"b\":[true,null,3]}"
        );
    }
}
