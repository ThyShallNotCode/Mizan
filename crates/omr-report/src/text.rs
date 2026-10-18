//! Output safety and formatting rules shared by every writer (spec §16.3, §20.1).
//!
//! - Bidi isolates U+2066–U+2069 are stripped from CSV and XLSX text.
//! - The formula guard applies to text-typed columns only: a leading `=`, `+`, `-`, `@`, TAB or
//!   CR gets a `'` prefix. Numeric columns are canonical numbers `-?[0-9]+(\.[0-9]+)?`, never
//!   prefixed, so negative scores stay numeric.
//! - Data exports are locale-neutral: ASCII digits and `.` decimals.

use omr_scoring::{Exact, RoundingStrategy, decimal_string};
use std::fmt::Write as _;

/// Removes the bidi isolate controls LRI, RLI, FSI and PDI (U+2066–U+2069).
pub fn strip_bidi(s: &str) -> String {
    s.chars()
        .filter(|c| !('\u{2066}'..='\u{2069}').contains(c))
        .collect()
}

/// Text-column guard: strips bidi isolates and prefixes `'` to a value that a spreadsheet
/// could read as a formula.
pub fn guard_text(s: &str) -> String {
    let s = strip_bidi(s);
    match s.chars().next() {
        Some('=' | '+' | '-' | '@' | '\t' | '\r') => format!("'{s}"),
        _ => s,
    }
}

/// True for a canonical number `-?[0-9]+(\.[0-9]+)?`.
pub fn is_canonical_number(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let (ip, fp) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    !ip.is_empty()
        && ip.bytes().all(|b| b.is_ascii_digit())
        && fp.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

/// An exact value with `decimals` places (half away from zero), canonical (`-0.00` → `0.00`).
pub fn exact_str(x: &Exact, decimals: u32) -> String {
    canon_zero(decimal_string(
        x,
        decimals,
        RoundingStrategy::MidpointAwayFromZero,
    ))
}

/// An advisory float with `decimals` places; `None` for NaN or infinities.
pub fn float_str(x: f64, decimals: usize) -> Option<String> {
    x.is_finite().then(|| canon_zero(format!("{x:.decimals$}")))
}

fn canon_zero(s: String) -> String {
    if s.starts_with('-') && s[1..].bytes().all(|b| b == b'0' || b == b'.') {
        s[1..].to_owned()
    } else {
        s
    }
}

/// HTML escaping for text and attribute values.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// XML-safe text for SVG (same escaping as HTML).
pub fn svg_escape(s: &str) -> String {
    html_escape(s)
}

/// Positional option code: 0 → `A`, 1 → `B`, … (§20.2: data exports use positional codes,
/// with the label set in metadata).
pub fn option_code(o: u32) -> char {
    char::from_u32(u32::from(b'A') + o.min(25)).unwrap_or('?')
}

/// Positional codes of an option mask, e.g. `0b101` → `AC`.
pub fn mask_code(mask: u32) -> String {
    let mut s = String::new();
    for o in 0..26 {
        if mask & (1 << o) != 0 {
            let _ = write!(s, "{}", option_code(o));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use omr_scoring::exact;

    #[test]
    fn formula_guard_on_text_only() {
        assert_eq!(guard_text("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(guard_text("+966"), "'+966");
        assert_eq!(guard_text("-x"), "'-x");
        assert_eq!(guard_text("@cmd"), "'@cmd");
        assert_eq!(guard_text("\tx"), "'\tx");
        assert_eq!(guard_text("Ahmad"), "Ahmad");
        assert_eq!(guard_text("\u{2068}=1\u{2069}"), "'=1");
        assert_eq!(guard_text("محمد\u{2066}"), "محمد");
    }

    #[test]
    fn canonical_numbers() {
        assert!(is_canonical_number("-12.50"));
        assert!(is_canonical_number("0"));
        assert!(!is_canonical_number("1e3"));
        assert!(!is_canonical_number("1."));
        assert!(!is_canonical_number(".5"));
        assert!(!is_canonical_number("١٢"));
        assert_eq!(exact_str(&exact(-1, 1000), 2), "0.00");
        assert_eq!(exact_str(&exact(-5, 4), 1), "-1.3");
        assert_eq!(exact_str(&exact(2, 3), 2), "0.67");
        assert_eq!(float_str(-0.0001, 3).unwrap(), "0.000");
        assert_eq!(float_str(f64::NAN, 3), None);
    }

    #[test]
    fn codes_and_escapes() {
        assert_eq!(mask_code(0b10101), "ACE");
        assert_eq!(option_code(1), 'B');
        assert_eq!(
            html_escape("<a href='x'>&\"</a>"),
            "&lt;a href=&#39;x&#39;&gt;&amp;&quot;&lt;/a&gt;"
        );
    }
}
