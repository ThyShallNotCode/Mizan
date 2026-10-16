//! Page code (spec §6.4): a fixed-width, 41-character Crockford Base32 payload printed twice per
//! side as a QR v3-M code, with an echo line under each copy.
//!
//! | # | Field | Bits | Chars |
//! |---|---|---|---|
//! | 1 | Scheme version (`1`) | 5 | 1 |
//! | 2 | Exam ID: first 40 bits of SHA-384("omr-exam-id-v1" ‖ exam UUID ‖ salt_u8) | 40 | 8 |
//! | 3 | Template revision | 10 | 2 |
//! | 4 | Variant/form (`00` = per-booklet shuffle, `01` = A, …) | 10 | 2 |
//! | 5 | Page (each side counts, 1-based) | 10 | 2 |
//! | 6 | Booklet serial (`000000` = generic copy) | 30 | 6 |
//! | 7 | Tag: first 96 bits of HMAC-SHA384(K_exam, "omr-pc1" ‖ chars 1–21), then 4 zero bits | 100 | 20 |
//!
//! The tag is compared in constant time. A non-zero pad fails decoding. The echo line's short
//! form (the tag's first 7 characters) is a misread checksum, never a verified tag.

use hmac::{Hmac, KeyInit, Mac};
use omr_model::{
    BookletSerial, ExamId, FormCode, PageNo, Scope, Severity, TemplateRevision,
    exceptions::Exception,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};
use std::fmt;
use thiserror::Error;

/// The Crockford Base32 alphabet (no I, L, O, U).
pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Payload length in characters.
pub const PAYLOAD_LEN: usize = 41;
/// Length of the authenticated body (chars 1–21).
pub const BODY_LEN: usize = 21;
/// Length of the tag field in characters (96 tag bits + 4 pad bits).
pub const TAG_CHARS: usize = 20;
/// Tag length in bytes (96 bits).
pub const TAG_BYTES: usize = 12;
/// Characters of the short form printed on the echo line (35 bits).
pub const SHORT_FORM_LEN: usize = 7;
/// The only scheme version defined.
pub const SCHEME_V1: u8 = 1;
/// Largest value of a 10-bit field.
pub const MAX_10BIT: u16 = 1_023;
/// Largest booklet serial (30 bits).
pub const MAX_SERIAL: u32 = (1 << 30) - 1;

const EXAM_ID_DOMAIN: &[u8] = b"omr-exam-id-v1";
const TAG_DOMAIN: &[u8] = b"omr-pc1";

/// The per-exam page-code secret K_exam (256 bits, §6.4, §19.2).
///
/// Never printed or logged: `Debug` is redacted and the type is not serialisable.
#[derive(Clone, PartialEq, Eq)]
pub struct ExamKey([u8; 32]);

impl ExamKey {
    /// Wraps 32 secret bytes.
    pub const fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    /// The secret bytes (for encrypted storage by `omr-store` only).
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ExamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExamKey(<redacted>)")
    }
}

/// The 40-bit exam ID: first 40 bits of SHA-384("omr-exam-id-v1" ‖ UUID bytes ‖ salt).
pub fn exam_id40(exam: &ExamId, salt: u8) -> u64 {
    let mut h = Sha384::new();
    h.update(EXAM_ID_DOMAIN);
    h.update(exam.0.as_bytes());
    h.update([salt]);
    let d = h.finalize();
    d[..5].iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
}

/// The routed fields of a page code (chars 1–21).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageCode {
    /// 40-bit exam ID (see [`exam_id40`]).
    pub exam_id40: u64,
    /// Template revision (10 bits).
    pub revision: TemplateRevision,
    /// Form (10 bits; 0 = per-booklet shuffle).
    pub form: FormCode,
    /// Page, 1-based (10 bits).
    pub page: PageNo,
    /// Booklet serial (30 bits; 0 = generic copy).
    pub serial: BookletSerial,
}

impl PageCode {
    /// Checks every field fits its width.
    pub fn check(&self) -> Result<(), PageCodeError> {
        if self.exam_id40 >= 1 << 40 {
            return Err(PageCodeError::FieldOutOfRange("exam_id40"));
        }
        if self.revision.0 > MAX_10BIT {
            return Err(PageCodeError::FieldOutOfRange("revision"));
        }
        if self.form.0 > MAX_10BIT {
            return Err(PageCodeError::FieldOutOfRange("form"));
        }
        if self.page.0 > MAX_10BIT {
            return Err(PageCodeError::FieldOutOfRange("page"));
        }
        if self.serial.0 > MAX_SERIAL {
            return Err(PageCodeError::FieldOutOfRange("serial"));
        }
        Ok(())
    }

    /// Chars 1–21 (scheme, exam ID, revision, form, page, serial).
    pub fn body(&self) -> Result<String, PageCodeError> {
        self.check()?;
        let mut s = String::with_capacity(PAYLOAD_LEN);
        push_b32(&mut s, u128::from(SCHEME_V1), 1);
        push_b32(&mut s, u128::from(self.exam_id40), 8);
        push_b32(&mut s, u128::from(self.revision.0), 2);
        push_b32(&mut s, u128::from(self.form.0), 2);
        push_b32(&mut s, u128::from(self.page.0), 2);
        push_b32(&mut s, u128::from(self.serial.0), 6);
        Ok(s)
    }

    /// The 96-bit tag over `body` (the first 12 bytes of the HMAC-SHA384).
    pub fn tag(&self, key: &ExamKey) -> Result<[u8; TAG_BYTES], PageCodeError> {
        Ok(tag_of_body(&self.body()?, key))
    }

    /// The full 41-character payload.
    pub fn encode(&self, key: &ExamKey) -> Result<String, PageCodeError> {
        let mut s = self.body()?;
        let tag = tag_of_body(&s, key);
        push_b32(&mut s, tag_bits(&tag), TAG_CHARS);
        debug_assert_eq!(s.len(), PAYLOAD_LEN);
        Ok(s)
    }
}

/// A structurally valid payload whose tag is **not yet verified**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedPageCode {
    /// The decoded fields.
    pub code: PageCode,
    /// The 96-bit tag as printed.
    pub tag: [u8; TAG_BYTES],
    body: String,
    tag_chars: String,
}

impl ParsedPageCode {
    /// Verifies the tag under `key` with a constant-time comparison.
    pub fn verify(&self, key: &ExamKey) -> Result<PageCode, PageCodeError> {
        let mut mac = new_mac(key);
        mac.update(TAG_DOMAIN);
        mac.update(self.body.as_bytes());
        mac.verify_truncated_left(&self.tag)
            .map_err(|_| PageCodeError::TagMismatch)?;
        Ok(self.code)
    }

    /// Chars 1–21.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// The short form: the tag's first 7 characters (a misread checksum only).
    pub fn short_form(&self) -> &str {
        &self.tag_chars[..SHORT_FORM_LEN]
    }
}

/// Parses a 41-character payload: length, alphabet, scheme and zero pad. Does **not** verify.
pub fn parse(s: &str) -> Result<ParsedPageCode, PageCodeError> {
    if s.len() != PAYLOAD_LEN || !s.is_ascii() {
        return Err(PageCodeError::WrongLength(s.chars().count()));
    }
    let vals = decode_chars(s)?;
    let scheme = vals[0];
    if scheme != SCHEME_V1 {
        return Err(PageCodeError::UnsupportedScheme(scheme));
    }
    let field = |from: usize, n: usize| -> u128 {
        vals[from..from + n]
            .iter()
            .fold(0u128, |acc, v| (acc << 5) | u128::from(*v))
    };
    let tag_field = field(BODY_LEN, TAG_CHARS);
    if tag_field & 0xF != 0 {
        return Err(PageCodeError::NonZeroPad);
    }
    let tag_u96 = tag_field >> 4;
    let mut tag = [0u8; TAG_BYTES];
    for (i, b) in tag.iter_mut().enumerate() {
        *b = (tag_u96 >> (8 * (TAG_BYTES - 1 - i))) as u8;
    }
    let code = PageCode {
        exam_id40: field(1, 8) as u64,
        revision: TemplateRevision(field(9, 2) as u16),
        form: FormCode(field(11, 2) as u16),
        page: PageNo(field(13, 2) as u16),
        serial: BookletSerial(field(15, 6) as u32),
    };
    Ok(ParsedPageCode {
        code,
        tag,
        body: s[..BODY_LEN].to_owned(),
        tag_chars: s[BODY_LEN..].to_owned(),
    })
}

/// Parses and verifies in one step.
pub fn decode_verified(s: &str, key: &ExamKey) -> Result<PageCode, PageCodeError> {
    parse(s)?.verify(key)
}

/// The short form of an encoded payload.
pub fn short_form(payload: &str) -> Result<&str, PageCodeError> {
    if payload.len() != PAYLOAD_LEN || !payload.is_ascii() {
        return Err(PageCodeError::WrongLength(payload.chars().count()));
    }
    Ok(&payload[BODY_LEN..BODY_LEN + SHORT_FORM_LEN])
}

/// The code part of the echo line: `1 X7K2QF3M 03 02 02 000CH1 K9QF2ZA` (§6.4).
pub fn echo_code(payload: &str) -> Result<String, PageCodeError> {
    let p = parse(payload)?;
    let b = p.body();
    Ok(format!(
        "{} {} {} {} {} {} {}",
        &b[0..1],
        &b[1..9],
        &b[9..11],
        &b[11..13],
        &b[13..15],
        &b[15..21],
        p.short_form()
    ))
}

/// The full echo line: friendly prefix, chars 1–21 and the short form.
pub fn echo_line(prefix: &str, payload: &str) -> Result<String, PageCodeError> {
    Ok(format!("{prefix} · {}", echo_code(payload)?))
}

/// Normalises OCR'd Crockford text: upper-cases, maps `O`→`0`, `I`/`L`→`1`, drops spaces,
/// hyphens and `·`. Returns `None` on any other character.
pub fn normalize_crockford(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let c = ch.to_ascii_uppercase();
        let mapped = match c {
            ' ' | '-' | '·' | '\t' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c if c.is_ascii() && ALPHABET.contains(&(c as u8)) => c,
            _ => return None,
        };
        out.push(mapped);
    }
    Some(out)
}

/// Checks an OCR'd echo-line code (chars 1–21 then the 7-char short form, any spacing) against
/// the short form recomputed under `key`. Success recovers routing only (IDN-001
/// `echo_line_recovered`); it never counts as a verified tag (§6.4).
pub fn check_echo_code(text: &str, key: &ExamKey) -> Result<PageCode, PageCodeError> {
    let norm = normalize_crockford(text).ok_or(PageCodeError::BadChar { pos: 0, ch: '?' })?;
    if norm.len() != BODY_LEN + SHORT_FORM_LEN {
        return Err(PageCodeError::WrongLength(norm.len()));
    }
    let body = &norm[..BODY_LEN];
    let tag = tag_of_body(body, key);
    let mut full = body.to_owned();
    push_b32(&mut full, tag_bits(&tag), TAG_CHARS);
    let expect = &full.as_bytes()[BODY_LEN..BODY_LEN + SHORT_FORM_LEN];
    let got = &norm.as_bytes()[BODY_LEN..];
    if !ct_eq(expect, got) {
        return Err(PageCodeError::TagMismatch);
    }
    Ok(parse(&full)?.code)
}

/// Why a payload was rejected.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PageCodeError {
    /// Not 41 ASCII characters.
    #[error("page code has {0} characters, expected {PAYLOAD_LEN}")]
    WrongLength(usize),
    /// A character outside the Crockford alphabet.
    #[error("invalid character {ch:?} at position {pos}")]
    BadChar {
        /// 0-based position.
        pos: usize,
        /// The character.
        ch: char,
    },
    /// Scheme version other than 1.
    #[error("unsupported page-code scheme {0}")]
    UnsupportedScheme(u8),
    /// The 4 pad bits after the 96-bit tag are not zero.
    #[error("non-zero pad bits after the tag")]
    NonZeroPad,
    /// The tag does not verify under this exam's key (foreign, forged or misread sheet).
    #[error("page-code tag does not verify")]
    TagMismatch,
    /// A field does not fit its width.
    #[error("page-code field {0} out of range")]
    FieldOutOfRange(&'static str),
}

impl PageCodeError {
    /// The exception a recognizer raises for this rejection: IDN-002 (BLOCKER, Page), since no
    /// MAC-valid copy was obtained (§6.4, §8.3). "Nothing decoded" (IDN-001) is the caller's case.
    pub fn exception(&self) -> Exception {
        Exception::must("IDN-002", Severity::Blocker, Scope::Page).with_detail(self.to_string())
    }
}

fn new_mac(key: &ExamKey) -> Hmac<Sha384> {
    // HMAC accepts keys of any length, so this cannot fail for a 32-byte key.
    match <Hmac<Sha384> as KeyInit>::new_from_slice(&key.0) {
        Ok(m) => m,
        Err(_) => unreachable!("HMAC takes keys of any length"),
    }
}

fn tag_of_body(body: &str, key: &ExamKey) -> [u8; TAG_BYTES] {
    let mut mac = new_mac(key);
    mac.update(TAG_DOMAIN);
    mac.update(body.as_bytes());
    let out = mac.finalize().into_bytes();
    let mut tag = [0u8; TAG_BYTES];
    tag.copy_from_slice(&out[..TAG_BYTES]);
    tag
}

/// The 100-bit tag field: 96 tag bits, big-endian, followed by 4 zero bits.
fn tag_bits(tag: &[u8; TAG_BYTES]) -> u128 {
    tag.iter().fold(0u128, |acc, b| (acc << 8) | u128::from(*b)) << 4
}

fn push_b32(s: &mut String, v: u128, chars: usize) {
    for i in (0..chars).rev() {
        let d = ((v >> (5 * i)) & 31) as usize;
        s.push(char::from(ALPHABET[d]));
    }
}

fn decode_chars(s: &str) -> Result<Vec<u8>, PageCodeError> {
    s.bytes()
        .enumerate()
        .map(|(pos, b)| {
            ALPHABET
                .iter()
                .position(|a| *a == b)
                .map(|p| p as u8)
                .ok_or(PageCodeError::BadChar {
                    pos,
                    ch: char::from(b),
                })
        })
        .collect()
}

/// Constant-time equality for equal-length slices (length itself is not secret).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> ExamKey {
        ExamKey::from_bytes([b; 32])
    }

    fn code() -> PageCode {
        PageCode {
            exam_id40: exam_id40(&ExamId(uuid::Uuid::from_u128(42)), 0),
            revision: TemplateRevision(3),
            form: FormCode(2),
            page: PageNo(2),
            serial: BookletSerial(12_345),
        }
    }

    #[test]
    fn req_a7_round_trip_and_length() {
        let k = key(7);
        let s = code().encode(&k).unwrap();
        assert_eq!(s.len(), PAYLOAD_LEN);
        assert!(s.bytes().all(|b| ALPHABET.contains(&b)));
        assert!(!s.contains(['I', 'L', 'O', 'U']));
        assert!(s.starts_with('1'));
        let back = decode_verified(&s, &k).unwrap();
        assert_eq!(back, code());
        // Fields land in their documented positions.
        assert_eq!(&s[9..11], "03");
        assert_eq!(&s[11..13], "02");
        assert_eq!(&s[13..15], "02");
        assert_eq!(&s[15..21], "000C1S");
        // The pad is zero: the last character carries one tag bit then 4 zero bits.
        assert!(matches!(s.as_bytes()[40], b'0' | b'G'));
    }

    #[test]
    fn req_a7_wrong_key_is_rejected() {
        let s = code().encode(&key(7)).unwrap();
        assert_eq!(decode_verified(&s, &key(8)), Err(PageCodeError::TagMismatch));
        let e = PageCodeError::TagMismatch.exception();
        assert_eq!(e.code, "IDN-002");
        assert_eq!(e.severity, Severity::Blocker);
    }

    #[test]
    fn req_a7_tampered_body_is_rejected() {
        let k = key(7);
        let s = code().encode(&k).unwrap();
        // Change the serial (a forged booklet number) but keep the old tag.
        let mut b = s.into_bytes();
        b[20] = if b[20] == b'2' { b'3' } else { b'2' };
        let t = String::from_utf8(b).unwrap();
        assert_eq!(parse(&t).map(|_| ()), Ok(()), "structurally valid");
        assert_eq!(decode_verified(&t, &k), Err(PageCodeError::TagMismatch));
    }

    #[test]
    fn req_a7_non_zero_pad_is_rejected() {
        let k = key(1);
        let s = code().encode(&k).unwrap();
        let mut b = s.into_bytes();
        let last = ALPHABET.iter().position(|a| *a == b[40]).unwrap();
        b[40] = ALPHABET[last | 1];
        let t = String::from_utf8(b).unwrap();
        assert_eq!(parse(&t), Err(PageCodeError::NonZeroPad));
    }

    #[test]
    fn structural_rejections() {
        assert_eq!(parse("1ABC"), Err(PageCodeError::WrongLength(4)));
        let s = code().encode(&key(1)).unwrap();
        let bad = s.replacen('1', "2", 1);
        assert_eq!(parse(&bad), Err(PageCodeError::UnsupportedScheme(2)));
        let mut b = s.into_bytes();
        b[5] = b'U';
        assert!(matches!(
            parse(&String::from_utf8(b).unwrap()),
            Err(PageCodeError::BadChar { pos: 5, ch: 'U' })
        ));
        let mut c = code();
        c.serial = BookletSerial(MAX_SERIAL + 1);
        assert_eq!(c.encode(&key(1)), Err(PageCodeError::FieldOutOfRange("serial")));
    }

    #[test]
    fn exam_id_depends_on_uuid_and_salt() {
        let e = ExamId(uuid::Uuid::from_u128(42));
        let a = exam_id40(&e, 0);
        assert!(a < 1 << 40);
        assert_ne!(a, exam_id40(&e, 1));
        assert_ne!(a, exam_id40(&ExamId(uuid::Uuid::from_u128(43)), 0));
    }

    #[test]
    fn echo_line_and_short_form() {
        let k = key(9);
        let s = code().encode(&k).unwrap();
        let line = echo_line("PHYS101-MT1 · Form B · Page 2/4", &s).unwrap();
        let sf = short_form(&s).unwrap();
        assert_eq!(sf.len(), SHORT_FORM_LEN);
        assert!(line.ends_with(sf));
        assert!(line.contains(&s[1..9]));
        // The echo code verifies against its short form, including OCR confusions.
        let echo = echo_code(&s).unwrap();
        assert_eq!(check_echo_code(&echo, &k), Ok(code()));
        let ocr = echo.replace('0', "O").to_lowercase();
        assert_eq!(check_echo_code(&ocr, &k), Ok(code()));
        // A misread digit fails the checksum.
        let mut bad = echo.into_bytes();
        let i = bad.iter().position(|c| *c == b'C').unwrap_or(4);
        bad[i] = if bad[i] == b'D' { b'E' } else { b'D' };
        assert_eq!(
            check_echo_code(&String::from_utf8(bad).unwrap(), &k),
            Err(PageCodeError::TagMismatch)
        );
    }

    #[test]
    fn debug_never_prints_the_key() {
        assert_eq!(format!("{:?}", key(0xAB)), "ExamKey(<redacted>)");
    }
}
