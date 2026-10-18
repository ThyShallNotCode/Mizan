//! Canonical JSON (RFC 8785 / JCS subset) and SHA-384 hashing of the layout manifest (§6.1).
//!
//! The manifest holds only integers, strings, booleans, arrays and objects, so the JCS number
//! rules reduce to plain integer formatting. Keys are sorted explicitly here (not by relying on
//! `serde_json`'s map type, whose order depends on a cargo feature another crate could enable).

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha384};
use std::fmt::Write as _;
use thiserror::Error;

/// A SHA-384 digest.
pub type Sha384Digest = [u8; 48];

/// Why a value has no canonical form here.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CanonicalError {
    /// The value contains a non-integer number (forbidden in hashed manifests, §6.1).
    #[error("non-integer number {0} in a hashed manifest")]
    Float(String),
    /// The value could not be serialised.
    #[error("serialisation failed: {0}")]
    Serialize(String),
}

/// Serialises `v` to canonical JSON text.
pub fn canonical_json<T: Serialize>(v: &T) -> Result<String, CanonicalError> {
    let value = serde_json::to_value(v).map_err(|e| CanonicalError::Serialize(e.to_string()))?;
    let mut out = String::new();
    write_value(&value, &mut out)?;
    Ok(out)
}

/// SHA-384 of the canonical JSON of `v`.
pub fn sha384_canonical<T: Serialize>(v: &T) -> Result<Sha384Digest, CanonicalError> {
    let text = canonical_json(v)?;
    Ok(sha384(text.as_bytes()))
}

/// SHA-384 of bytes.
pub fn sha384(bytes: &[u8]) -> Sha384Digest {
    let d = Sha384::digest(bytes);
    let mut out = [0u8; 48];
    out.copy_from_slice(&d);
    out
}

/// Lower-case hex of a digest.
pub fn hex(d: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(d)
}

fn write_value(v: &Value, out: &mut String) -> Result<(), CanonicalError> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let _ = write!(out, "{i}");
            } else if let Some(u) = n.as_u64() {
                let _ = write!(out, "{u}");
            } else {
                return Err(CanonicalError::Float(n.to_string()));
            }
        }
        Value::String(s) => write_string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(x, out)?;
            }
            out.push(']');
        }
        Value::Object(m) => {
            // JCS sorts keys by UTF-16 code units.
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                if let Some(x) = m.get(k) {
                    write_value(x, out)?;
                }
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sorted_and_compact() {
        let v = json!({"b": 1, "a": [true, null, -3], "c": {"z": "x\"y\n", "y": "أ"}});
        assert_eq!(
            canonical_json(&v).unwrap(),
            r#"{"a":[true,null,-3],"b":1,"c":{"y":"أ","z":"x\"y\n"}}"#
        );
    }

    #[test]
    fn floats_are_refused() {
        assert!(matches!(
            canonical_json(&json!({"x": 1.5})),
            Err(CanonicalError::Float(_))
        ));
    }

    #[test]
    fn hash_is_stable() {
        let a = sha384_canonical(&json!({"a": 1, "b": 2})).unwrap();
        let b = sha384_canonical(&json!({"b": 2, "a": 1})).unwrap();
        assert_eq!(a, b);
        assert_eq!(hex(&a).len(), 96);
    }
}
