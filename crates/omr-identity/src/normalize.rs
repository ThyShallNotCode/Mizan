//! Text normalisation for identity keys (§9.2 rule 3) and names (§9.3).
//!
//! Derived keys are computed on a copy; displayed and exported text never changes.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Format and bidi controls removed from keys and names (§9.3 step 7) plus the soft hyphen.
fn is_format_control(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// Arabic-Indic (U+0660–0669) and Extended Arabic-Indic (U+06F0–06F9) digits → ASCII.
pub fn fold_digit(c: char) -> char {
    match c {
        '\u{0660}'..='\u{0669}' => char::from(b'0' + (c as u32 - 0x0660) as u8),
        '\u{06F0}'..='\u{06F9}' => char::from(b'0' + (c as u32 - 0x06F0) as u8),
        _ => c,
    }
}

/// Canonical ID key (§9.2 rule 3; also the `student_pid` input, §14.9): NFKC (full-width
/// digits → ASCII, NBSP → space), Arabic-Indic and Extended digits → ASCII, format and bidi
/// controls stripped, trimmed, upper-cased. Leading zeros are kept: IDs are text end to end.
pub fn normalize_id(raw: &str) -> String {
    let s: String = raw
        .nfkc()
        .map(fold_digit)
        .filter(|c| !is_format_control(*c))
        .collect();
    s.trim().to_uppercase()
}

/// The ID key reduced to ASCII digits only when it consists of digits (after
/// [`normalize_id`]) possibly separated by spaces or hyphens; `None` otherwise.
pub fn digits_only(raw: &str) -> Option<String> {
    let n = normalize_id(raw);
    let digits: String = n.chars().filter(|c| !matches!(c, ' ' | '-')).collect();
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// Arabic letter folding of §9.3 step 4.
fn fold_letter(c: char) -> char {
    match c {
        '\u{0671}' => '\u{0627}', // ٱ → ا
        '\u{0649}' => '\u{064A}', // ى → ي
        '\u{0629}' => '\u{0647}', // ة → ه
        '\u{06A9}' => '\u{0643}', // Persian ک → ك
        '\u{06CC}' => '\u{064A}', // Persian ی → ي
        _ => c,
    }
}

fn is_arabic_mark(c: char) -> bool {
    matches!(c,
        '\u{0610}'..='\u{061A}' | '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{06D6}'..='\u{06ED}')
}

/// Name matching key (§9.3 steps 1–7): NFKD; marks removed (so أ إ آ ؤ ئ fold to ا ا ا و ي);
/// tatweel removed; Arabic letter map; digits folded; NFC; case folded; controls removed;
/// whitespace collapsed.
///
/// Deviation: case folding is Rust's `to_lowercase` plus ß/ẞ → "ss" instead of ICU4X full case
/// folding (identical for Arabic and Latin names apart from rare special cases).
pub fn normalize_name(raw: &str) -> String {
    let stage: String = raw
        .nfkd()
        .filter(|c| !is_combining_mark(*c) && !is_arabic_mark(*c))
        .filter(|c| *c != '\u{0640}')
        .map(fold_letter)
        .map(fold_digit)
        .filter(|c| !is_format_control(*c))
        .collect();
    let folded: String = stage
        .nfc()
        .flat_map(|c| match c {
            'ß' | 'ẞ' => "ss".chars().collect::<Vec<_>>(),
            c => c.to_lowercase().collect(),
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Name tokens of a normalised name (split on whitespace, hyphens and apostrophes).
pub fn name_tokens(normalized: &str) -> Vec<String> {
    normalized
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '\'' | '’' | '.' | ','))
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Levenshtein distance between token sequences (tokens compare as whole strings).
pub fn token_edit_distance(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ta) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, tb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ta != tb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_keys_keep_leading_zeros_and_fold_digits() {
        assert_eq!(normalize_id("  0012345 "), "0012345");
        // Arabic-Indic, Extended Arabic-Indic and full-width digits.
        assert_eq!(normalize_id("٢٢٢٠٠٠١٢٣٤"), "2220001234");
        assert_eq!(normalize_id("۲۲۲۰۰۰۱۲۳۴"), "2220001234");
        assert_eq!(normalize_id("２２２０００１２３４"), "2220001234");
        // Bidi controls and NBSP; upper-casing.
        assert_eq!(normalize_id("\u{200F}s12\u{00A0}34\u{202C}"), "S12 34");
        assert_eq!(digits_only("222-000 1234").as_deref(), Some("2220001234"));
        assert_eq!(digits_only("22A"), None);
        assert_eq!(digits_only(""), None);
    }

    #[test]
    fn arabic_names_fold_per_spec() {
        // Hamza forms, diacritics, tatweel, taa marbuta, alef maqsura.
        assert_eq!(normalize_name("أحمَد"), normalize_name("احمد"));
        assert_eq!(normalize_name("إبراهيم"), normalize_name("ابراهيم"));
        assert_eq!(normalize_name("فاطمة"), normalize_name("فاطمه"));
        assert_eq!(normalize_name("مصطفى"), normalize_name("مصطفي"));
        assert_eq!(normalize_name("محـــمد"), normalize_name("محمد"));
        assert_eq!(
            normalize_name("ﻣﺤﻤﺪ"),
            normalize_name("محمد"),
            "presentation forms"
        );
        assert_eq!(normalize_name("  José   MÜLLER "), "jose muller");
        assert_eq!(normalize_name("Straße"), "strasse");
    }

    #[test]
    fn token_distance() {
        let t = |s: &str| name_tokens(&normalize_name(s));
        assert_eq!(
            token_edit_distance(&t("Ali Hassan Omar"), &t("Ali Omar")),
            1
        );
        assert_eq!(token_edit_distance(&t("Ali Hassan"), &t("Ali Hassan")), 0);
        assert_eq!(token_edit_distance(&t("Ali Hassan"), &t("Sara Noor")), 2);
    }
}
