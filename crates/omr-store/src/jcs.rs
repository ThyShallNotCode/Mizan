//! RFC 8785 JSON Canonicalization Scheme (JCS) and SHA-384 helpers (spec §14.3, §14.4).
//!
//! Event payloads, content-addressed rows (template revisions, policies, mapping sets, roster
//! snapshots, pinned bodies) and checkpoint messages are hashed over their JCS form:
//! - object members sorted by the UTF-16 code units of their names;
//! - no insignificant whitespace;
//! - strings escaped minimally (`"`, `\`, and control characters below U+0020 with the short
//!   forms `\b \t \n \f \r` or `\u00xx`), everything else as literal UTF-8;
//! - numbers in the ECMAScript `Number.prototype.toString` form.
//!
//! Integers outside ±2⁵³ are refused (JCS reads numbers as IEEE doubles), as are NaN and
//! infinities. The spec keeps decimals as strings (§14.4), so floats are rare; when present
//! they are written with the shortest round-trip digits, which is what ECMAScript prints.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha384};
use std::fmt::Write as _;
use thiserror::Error;

/// Largest integer JCS can carry exactly (2⁵³).
pub const MAX_SAFE_INTEGER: u64 = 1 << 53;

/// Why a value has no JCS form.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum JcsError {
    /// An integer outside ±2⁵³ (encode it as a string).
    #[error("integer {0} is outside the JCS-safe range ±2^53; encode it as a string")]
    UnsafeInteger(String),
    /// NaN or an infinity.
    #[error("non-finite number in canonical JSON")]
    NonFinite,
    /// The value could not be serialised to JSON.
    #[error("serialisation failed: {0}")]
    Serialize(String),
}

/// Canonical JSON text of any serialisable value.
pub fn to_jcs<T: Serialize + ?Sized>(v: &T) -> Result<String, JcsError> {
    let value = serde_json::to_value(v).map_err(|e| JcsError::Serialize(e.to_string()))?;
    value_to_jcs(&value)
}

/// Canonical JSON text of a [`Value`].
pub fn value_to_jcs(v: &Value) -> Result<String, JcsError> {
    let mut out = String::new();
    write_value(v, &mut out)?;
    Ok(out)
}

/// SHA-384 (lower-case hex) of the canonical JSON of `v`.
pub fn jcs_sha384_hex<T: Serialize + ?Sized>(v: &T) -> Result<String, JcsError> {
    Ok(sha384_hex(to_jcs(v)?.as_bytes()))
}

/// SHA-384 of bytes.
pub fn sha384(bytes: &[u8]) -> [u8; 48] {
    let d = Sha384::digest(bytes);
    let mut out = [0u8; 48];
    out.copy_from_slice(&d);
    out
}

/// Lower-case hex SHA-384 of bytes.
pub fn sha384_hex(bytes: &[u8]) -> String {
    hex::encode(sha384(bytes))
}

fn write_value(v: &Value, out: &mut String) -> Result<(), JcsError> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(n, out)?,
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            // RFC 8785 §3.2.3: sort by UTF-16 code units, not by UTF-8 bytes.
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (k, key) in keys.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&map[key.as_str()], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(n: &serde_json::Number, out: &mut String) -> Result<(), JcsError> {
    if let Some(i) = n.as_i64() {
        if i.unsigned_abs() > MAX_SAFE_INTEGER {
            return Err(JcsError::UnsafeInteger(i.to_string()));
        }
        let _ = write!(out, "{i}");
        return Ok(());
    }
    if let Some(u) = n.as_u64() {
        if u > MAX_SAFE_INTEGER {
            return Err(JcsError::UnsafeInteger(u.to_string()));
        }
        let _ = write!(out, "{u}");
        return Ok(());
    }
    let f = n.as_f64().ok_or(JcsError::NonFinite)?;
    out.push_str(&es_number(f)?);
    Ok(())
}

/// ECMAScript `Number.prototype.toString` for a finite double (RFC 8785 §3.2.2.3).
pub fn es_number(f: f64) -> Result<String, JcsError> {
    if !f.is_finite() {
        return Err(JcsError::NonFinite);
    }
    if f == 0.0 {
        return Ok("0".into()); // also −0
    }
    let a = f.abs();
    if (1e-6..1e21).contains(&a) {
        // Rust's shortest round-trip digits in positional notation match ECMAScript here.
        return Ok(format!("{f}"));
    }
    // Exponential form: ECMAScript writes an explicit sign on the exponent.
    let s = format!("{f:e}");
    Ok(match s.split_once('e') {
        Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
        _ => s,
    })
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
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
    fn sorts_keys_and_strips_whitespace() {
        let v = json!({"b": 1, "a": [true, null, "x"], "c": {"z": 0, "y": -3}});
        assert_eq!(
            value_to_jcs(&v).unwrap(),
            r#"{"a":[true,null,"x"],"b":1,"c":{"y":-3,"z":0}}"#
        );
    }

    #[test]
    fn utf16_key_order() {
        // UTF-16 and UTF-8 orders differ for U+E000..U+FFFF against supplementary characters:
        // 😀 is the surrogate pair D83D DE00 < FB01 in UTF-16, but F0… > EF… in UTF-8.
        let v = json!({"\u{1F600}": 1, "\u{FB01}": 2});
        assert_eq!(value_to_jcs(&v).unwrap(), "{\"\u{1F600}\":1,\"\u{FB01}\":2}");
    }

    #[test]
    fn string_escapes() {
        let v = json!("a\"b\\c\n\u{01}\u{7f}é/");
        assert_eq!(value_to_jcs(&v).unwrap(), "\"a\\\"b\\\\c\\n\\u0001\u{7f}é/\"");
    }

    #[test]
    fn numbers_follow_ecmascript() {
        assert_eq!(es_number(1.0).unwrap(), "1");
        assert_eq!(es_number(-0.0).unwrap(), "0");
        assert_eq!(es_number(0.5).unwrap(), "0.5");
        assert_eq!(es_number(1e21).unwrap(), "1e+21");
        assert_eq!(es_number(1.5e-7).unwrap(), "1.5e-7");
        assert_eq!(es_number(123456789.125).unwrap(), "123456789.125");
        assert_eq!(es_number(0.000001).unwrap(), "0.000001");
        assert!(es_number(f64::NAN).is_err());
        // RFC 8785 Appendix B samples.
        assert_eq!(es_number(f64::from_bits(0x4340000000000000)).unwrap(), "9007199254740992");
        assert_eq!(es_number(f64::from_bits(0x44b52d02c7e14af6)).unwrap(), "1e+23");
        assert_eq!(es_number(f64::from_bits(0x3eb0c6f7a0b5ed8d)).unwrap(), "0.000001");
        assert_eq!(es_number(f64::from_bits(0x3eb0c6f7a0b5ed8c)).unwrap(), "9.999999999999997e-7");
    }

    #[test]
    fn unsafe_integers_are_refused() {
        assert!(value_to_jcs(&json!(9_007_199_254_740_993u64)).is_err());
        assert!(value_to_jcs(&json!(9_007_199_254_740_992u64)).is_ok());
    }

    #[test]
    fn sha384_is_hex() {
        assert_eq!(sha384_hex(b"").len(), 96);
        assert!(
            sha384_hex(b"abc").starts_with("cb00753f45a35e8bb5a03d699ac65007272c32ab0eded163")
        );
    }
}
