//! Bounded decoding of PDF stream filter chains (spec §7.4 input limits; §8.12 row 31 "decoder
//! bomb": reject the page, the batch continues).
//!
//! `hayro-syntax`'s `Stream::decoded()` inflates without an output bound, so a small Flate
//! stream (a page content stream, a Form XObject, an Indexed palette or an image behind a
//! filter chain) could expand to gigabytes and abort the process, which no per-page isolation
//! can catch. Every stream this crate decodes goes through [`decode_filters`] instead: each
//! stage's output is capped before it is allocated and an over-limit stream is
//! [`StreamError::Bound`] (the page is refused with CAP-001).
//!
//! Supported filters: `FlateDecode` (zlib or raw deflate, with the PNG/TIFF predictors),
//! `ASCIIHexDecode`, `ASCII85Decode`, `RunLengthDecode`. Anything else (LZW, Crypt, image
//! codecs) is [`StreamError::Unsupported`]; image codecs are handled by the caller after the
//! pre-filters.

use crate::PdfError;
use crate::flate::{self, PredictorParams};
use hayro_syntax::object::Dict;
use miniz_oxide::inflate::{
    TINFLStatus, decompress_to_vec_with_limit, decompress_to_vec_zlib_with_limit,
};

/// Why a stream could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamError {
    /// The decoded output exceeds the byte bound (decompression bomb or oversized stream).
    Bound(usize),
    /// Damaged data.
    Bad(String),
    /// A filter this decoder does not handle.
    Unsupported(String),
}

impl From<StreamError> for PdfError {
    fn from(e: StreamError) -> Self {
        match e {
            StreamError::Bound(n) => PdfError::Corrupt(format!(
                "stream output exceeds the {n}-byte decode bound (decompression bomb?)"
            )),
            StreamError::Bad(s) => PdfError::Corrupt(s),
            StreamError::Unsupported(s) => PdfError::Unsupported(s),
        }
    }
}

/// Decoded bytes plus whether a Flate stage was damaged (truncated, bad checksum) but yielded
/// output.
pub(crate) struct Decoded {
    pub(crate) data: Vec<u8>,
    pub(crate) damaged: bool,
}

/// Applies `filters` (outermost first) to `raw`, never producing more than `limit` bytes at
/// any stage. `parms[i]` is the `/DecodeParms` dictionary of `filters[i]`, if any.
pub(crate) fn decode_filters(
    raw: &[u8],
    filters: &[String],
    parms: &[Option<Dict<'_>>],
    limit: usize,
) -> Result<Decoded, StreamError> {
    if filters.is_empty() {
        if raw.len() > limit {
            return Err(StreamError::Bound(limit));
        }
        return Ok(Decoded {
            data: raw.to_vec(),
            damaged: false,
        });
    }
    let mut cur: Option<Vec<u8>> = None;
    let mut damaged = false;
    for (i, f) in filters.iter().enumerate() {
        let input: &[u8] = cur.as_deref().unwrap_or(raw);
        let p = parms.get(i).cloned().flatten();
        let out = match f.as_str() {
            "FlateDecode" => {
                let (data, dmg) = inflate_bounded(input, limit)?;
                damaged |= dmg;
                unpredict(data, p.as_ref())?
            }
            "ASCIIHexDecode" => ascii_hex(input)?,
            "ASCII85Decode" => ascii85(input, limit)?,
            "RunLengthDecode" => run_length(input, limit)?,
            other => return Err(StreamError::Unsupported(format!("filter {other}"))),
        };
        if out.len() > limit {
            return Err(StreamError::Bound(limit));
        }
        cur = Some(out);
    }
    Ok(Decoded {
        data: cur.unwrap_or_default(),
        damaged,
    })
}

/// Zlib, then raw deflate; at most `limit` bytes. A damaged stream keeps its longest prefix.
fn inflate_bounded(input: &[u8], limit: usize) -> Result<(Vec<u8>, bool), StreamError> {
    match decompress_to_vec_zlib_with_limit(input, limit) {
        Ok(v) => Ok((v, false)),
        Err(e) if e.status == TINFLStatus::HasMoreOutput => Err(StreamError::Bound(limit)),
        Err(e) => match decompress_to_vec_with_limit(input, limit) {
            Ok(v) => Ok((v, false)),
            Err(e2) if e2.status == TINFLStatus::HasMoreOutput => Err(StreamError::Bound(limit)),
            Err(e2) => {
                let best = if e2.output.len() > e.output.len() {
                    e2.output
                } else {
                    e.output
                };
                if best.is_empty() {
                    Err(StreamError::Bad(
                        "FlateDecode stream is not decodable".into(),
                    ))
                } else {
                    Ok((best, true))
                }
            }
        },
    }
}

/// The Flate predictor named in `/DecodeParms`, validated (a bad geometry never panics).
fn unpredict(data: Vec<u8>, p: Option<&Dict<'_>>) -> Result<Vec<u8>, StreamError> {
    let Some(p) = p else {
        return Ok(data);
    };
    let pp = PredictorParams {
        predictor: p.get::<i32>(b"Predictor").unwrap_or(1),
        colors: p.get::<u32>(b"Colors").unwrap_or(1),
        bits_per_component: p.get::<u32>(b"BitsPerComponent").unwrap_or(8),
        columns: p.get::<u32>(b"Columns").unwrap_or(1),
    };
    if pp.predictor <= 1 {
        return Ok(data);
    }
    flate::validate_predictor(&pp).map_err(|e| StreamError::Bad(e.to_string()))?;
    flate::unpredict(data, &pp).map_err(|e| StreamError::Bad(e.to_string()))
}

fn ascii_hex(input: &[u8]) -> Result<Vec<u8>, StreamError> {
    let mut out = Vec::with_capacity(input.len() / 2 + 1);
    let mut hi: Option<u8> = None;
    for &c in input {
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            b'>' => break,
            c if c.is_ascii_whitespace() || c == 0 => continue,
            _ => return Err(StreamError::Bad("ASCIIHexDecode: invalid character".into())),
        };
        match hi.take() {
            Some(h) => out.push(h << 4 | v),
            None => hi = Some(v),
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    Ok(out)
}

fn ascii85(input: &[u8], limit: usize) -> Result<Vec<u8>, StreamError> {
    let mut out = Vec::with_capacity((input.len() / 5 * 4 + 4).min(limit));
    let mut group = [0u32; 5];
    let mut n = 0usize;
    let body = input.strip_prefix(b"<~").unwrap_or(input);
    for &c in body {
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = u32::from(c - b'!');
                n += 1;
                if n == 5 {
                    let v = group
                        .iter()
                        .try_fold(0u32, |a, &d| a.checked_mul(85)?.checked_add(d))
                        .ok_or_else(|| StreamError::Bad("ASCII85Decode: group overflow".into()))?;
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            c if c.is_ascii_whitespace() || c == 0 => continue,
            _ => return Err(StreamError::Bad("ASCII85Decode: invalid character".into())),
        }
        if out.len() > limit {
            return Err(StreamError::Bound(limit));
        }
    }
    if n == 1 {
        return Err(StreamError::Bad("ASCII85Decode: dangling character".into()));
    }
    if n > 1 {
        for g in group.iter_mut().skip(n) {
            *g = 84; // pad with 'u'
        }
        let v = group
            .iter()
            .try_fold(0u32, |a, &d| a.checked_mul(85)?.checked_add(d))
            .ok_or_else(|| StreamError::Bad("ASCII85Decode: group overflow".into()))?;
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    Ok(out)
}

fn run_length(input: &[u8], limit: usize) -> Result<Vec<u8>, StreamError> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let l = input[i];
        i += 1;
        match l {
            128 => break,
            0..=127 => {
                let n = usize::from(l) + 1;
                let end = (i + n).min(input.len());
                out.extend_from_slice(&input[i..end]);
                i = end;
            }
            _ => {
                let Some(&b) = input.get(i) else { break };
                i += 1;
                out.resize(out.len() + 257 - usize::from(l), b);
            }
        }
        if out.len() > limit {
            return Err(StreamError::Bound(limit));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::deflate::compress_to_vec_zlib;

    fn names(f: &[&str]) -> Vec<String> {
        f.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn ascii_filters_round_trip() {
        assert_eq!(ascii_hex(b"48 65 6c6C6f>").unwrap(), b"Hello");
        assert_eq!(ascii_hex(b"414>").unwrap(), b"A@");
        // "Man " → 9jqo^ ; 'z' is four zero bytes; a final partial group.
        assert_eq!(ascii85(b"9jqo^z~>", 100).unwrap(), b"Man \0\0\0\0");
        assert_eq!(
            ascii85(b"<~87cURD]i,\"Ebo7~>", 100).unwrap(),
            b"Hello World"
        );
        assert_eq!(
            ascii85(b"uuuuu", 100).unwrap_err(),
            StreamError::Bad("ASCII85Decode: group overflow".into())
        );
        assert_eq!(
            run_length(&[2, b'a', b'b', b'c', 254, b'x', 128], 100).unwrap(),
            b"abcxxx"
        );
    }

    #[test]
    fn every_stage_is_bounded() {
        let big = vec![b' '; 4 << 20];
        let z = compress_to_vec_zlib(&big, 6);
        assert!(z.len() < 64 << 10, "a compact bomb");
        let f = names(&["FlateDecode"]);
        assert_eq!(
            decode_filters(&z, &f, &[None], 1 << 20).err(),
            Some(StreamError::Bound(1 << 20))
        );
        let ok = decode_filters(&z, &f, &[None], 8 << 20).unwrap();
        assert_eq!(ok.data.len(), big.len());
        // RunLength expands 64× at most per pair and is checked as it goes.
        let rl: Vec<u8> = std::iter::repeat_n([129u8, b'x'], 20_000)
            .flatten()
            .collect();
        assert_eq!(
            decode_filters(&rl, &names(&["RunLengthDecode"]), &[None], 1 << 20).err(),
            Some(StreamError::Bound(1 << 20))
        );
        // ASCII85 'z' expands 4×.
        let zz = vec![b'z'; 1 << 20];
        assert_eq!(
            decode_filters(&zz, &names(&["ASCII85Decode"]), &[None], 1 << 20).err(),
            Some(StreamError::Bound(1 << 20))
        );
        // Chains: hex-encoded Flate.
        let small = compress_to_vec_zlib(b"q 1 0 0 1 0 0 cm Q", 6);
        let hex: String = small.iter().map(|b| format!("{b:02X}")).collect();
        let d = decode_filters(
            hex.as_bytes(),
            &names(&["ASCIIHexDecode", "FlateDecode"]),
            &[None, None],
            1 << 10,
        )
        .unwrap();
        assert_eq!(d.data, b"q 1 0 0 1 0 0 cm Q");
        assert_eq!(
            decode_filters(b"", &names(&["LZWDecode"]), &[None], 10).err(),
            Some(StreamError::Unsupported("filter LZWDecode".into()))
        );
    }
}
