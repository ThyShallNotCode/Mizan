//! Review keyboard map (spec §13.3, DESIGN §9.4) with physical-key positions.
//!
//! Slint 1.18 delivers the *produced* text of a key, not its scan code (ADR-001 S4 is still
//! unverified for Slint). To keep bindings on physical positions, the produced text is mapped
//! back through the layouts the university uses: a key pressed under the Arabic (101) layout
//! produces e.g. "ش" on the A position, so "ش" selects option A. Option keys also accept typed
//! Arabic option letters in abjad order (أ ب ج د هـ …, §13.3, §20.2), and Arabic-Indic digits.
//!
//! Resolution order for an option key: Latin letter, digit, abjad letter, Arabic (101) layout
//! position. For items with up to five options (A–E) the readings never disagree. On wider
//! items three keys overlap (ب sits on F, ا on H, ه on I); there the abjad reading wins, because
//! the printed label set is what the reviewer reads.

use slint::platform::Key;

/// What the focused review item expects from option keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyContext {
    /// Mark item with `options` options: letters and digits set the answer.
    Mark { options: usize },
    /// Identity item with `candidates` candidates: `1`–`n` choose, `N` / `D` (§13.3).
    Identity { candidates: usize },
    /// Page- or exam-level item: no option keys.
    Notice,
}

/// A review command produced by a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKey {
    /// Set the answer to option `n` (0-based position).
    Option(u8),
    /// Choose identity candidate `n` (0-based).
    Candidate(u8),
    /// `N` in identity context.
    NotOnRoster,
    /// `D` in identity context.
    Duplicate,
    /// Enter: accept the suggestion, or commit the chosen value.
    Accept,
    /// Shift+Enter: apply to the rest of the group.
    ApplyToGroup,
    Next,
    Previous,
    /// Backspace.
    Blank,
    /// Ctrl+M.
    Multi,
    /// Ctrl+U.
    Unreadable,
    /// Ctrl+D.
    Defer,
    /// Ctrl+E.
    Escalate,
    /// Ctrl+R.
    Rescan,
    /// Ctrl+Z.
    Undo,
    /// Space.
    ToggleOverlay,
    /// F2.
    ToggleOriginal,
    /// F1.
    Help,
    /// "/" or Ctrl+F.
    Search,
    ZoomIn,
    ZoomOut,
}

/// A global (any screen) command produced by a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalKey {
    /// Ctrl+1 … Ctrl+6: jump to the n-th screen (0-based); Ctrl+0 / Ctrl+, opens Settings.
    Screen(u8),
    Settings,
    /// Escape.
    Dismiss,
}

/// A Ctrl chord whose meaning depends on the screen showing (DESIGN §9): Ctrl+N and Ctrl+K on
/// Exams, Ctrl+Space and Ctrl+R on Scan, Ctrl+P on Results. Letters are physical positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    /// Ctrl + the letter at this physical position (`'a'`…`'z'`).
    Letter(char),
    /// Ctrl+Space.
    Space,
}

/// Physical key positions A–Z under the Arabic (101) layout (Windows; macOS "Arabic - PC").
const ARABIC_101: [&str; 26] = [
    "ش", "لا", "ؤ", "ي", "ث", "ب", "ل", "ا", "ه", "ت", "ن", "م", "ة", "ى", "خ", "ح", "ض", "ق", "س",
    "ف", "ع", "ر", "ص", "ء", "غ", "ئ",
];

/// Arabic option letters in abjad order (the template's Arabic label set, §6.5).
fn abjad_index(c: char) -> Option<usize> {
    Some(match c {
        'ا' | 'أ' | 'إ' | 'آ' => 0,
        'ب' => 1,
        'ج' => 2,
        'د' => 3,
        'ه' | 'ة' => 4,
        'و' => 5,
        'ز' => 6,
        'ح' => 7,
        'ط' => 8,
        'ي' | 'ى' => 9,
        _ => return None,
    })
}

/// Physical letter position (0 = A … 25 = Z) of a produced text under Latin or Arabic (101).
pub fn physical_letter(text: &str) -> Option<usize> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if first.is_ascii_alphabetic() && chars.next().is_none() {
        return Some((first.to_ascii_lowercase() as u8 - b'a') as usize);
    }
    // "لا" arrives as two code points (or as the U+FEFB ligature on some systems)
    let t = if text == "\u{FEFB}" { "لا" } else { text };
    ARABIC_101.iter().position(|&a| a == t)
}

/// Digit value of a produced text: Latin, Arabic-Indic or Extended Arabic-Indic.
pub fn digit(text: &str) -> Option<u32> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match c {
        '0'..='9' => c.to_digit(10),
        '\u{0660}'..='\u{0669}' => Some(c as u32 - 0x0660),
        '\u{06F0}'..='\u{06F9}' => Some(c as u32 - 0x06F0),
        _ => None,
    }
}

/// Option position selected by a produced text, for an item with `options` options.
pub fn option_for(text: &str, options: usize) -> Option<u8> {
    let within = |i: usize| (i < options).then_some(i as u8);
    let mut chars = text.chars();
    let c = chars.next()?;
    let single = chars.next().is_none();
    if single && c.is_ascii_alphabetic() {
        return within((c.to_ascii_lowercase() as u8 - b'a') as usize);
    }
    if let Some(d) = digit(text) {
        return if d == 0 { None } else { within(d as usize - 1) };
    }
    if single && let Some(i) = abjad_index(c).and_then(within) {
        return Some(i);
    }
    physical_letter(text).and_then(within)
}

fn key_char(k: Key) -> char {
    char::from(k)
}

fn is_key(text: &str, k: Key) -> bool {
    let mut chars = text.chars();
    chars.next() == Some(key_char(k)) && chars.next().is_none()
}

/// Maps a key press in the review frame to a command. `single_key` is the user's
/// "single-key shortcuts" setting (WCAG 2.1.4): when off, only non-character keys and Ctrl
/// chords work.
pub fn review_key(
    text: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    context: KeyContext,
    single_key: bool,
) -> Option<ReviewKey> {
    if alt {
        return None; // no Alt+letter bindings (Windows menu mnemonics, §13.3)
    }
    if is_key(text, Key::Return) || text == "\r" {
        return Some(if shift {
            ReviewKey::ApplyToGroup
        } else {
            ReviewKey::Accept
        });
    }
    if ctrl {
        return match physical_letter(text).map(|i| (b'a' + i as u8) as char) {
            Some('m') => Some(ReviewKey::Multi),
            Some('u') => Some(ReviewKey::Unreadable),
            Some('d') => Some(ReviewKey::Defer),
            Some('e') => Some(ReviewKey::Escalate),
            Some('r') => Some(ReviewKey::Rescan),
            Some('z') => Some(ReviewKey::Undo),
            Some('f') => Some(ReviewKey::Search),
            _ => None,
        };
    }
    if is_key(text, Key::DownArrow) {
        return Some(ReviewKey::Next);
    }
    if is_key(text, Key::UpArrow) {
        return Some(ReviewKey::Previous);
    }
    if is_key(text, Key::Backspace) {
        return Some(ReviewKey::Blank);
    }
    if is_key(text, Key::Space) {
        return Some(ReviewKey::ToggleOverlay);
    }
    if is_key(text, Key::F2) {
        return Some(ReviewKey::ToggleOriginal);
    }
    if is_key(text, Key::F1) {
        return Some(ReviewKey::Help);
    }
    if !single_key {
        return None;
    }
    // options letters reach J (position 9) and K (10) only on very wide items
    let letter_options = match context {
        KeyContext::Mark { options } => {
            if let Some(i) = option_for(text, options) {
                return Some(ReviewKey::Option(i));
            }
            options
        }
        KeyContext::Identity { candidates } => {
            if let Some(d) = digit(text)
                && d >= 1
                && (d as usize) <= candidates
            {
                return Some(ReviewKey::Candidate(d as u8 - 1));
            }
            match physical_letter(text) {
                Some(13) => return Some(ReviewKey::NotOnRoster),
                Some(3) => return Some(ReviewKey::Duplicate),
                _ => {}
            }
            0
        }
        KeyContext::Notice => 0,
    };
    // J / K navigate only when no option uses them (positions 9 and 10)
    match physical_letter(text) {
        Some(9) if letter_options <= 9 => return Some(ReviewKey::Next),
        Some(10) if letter_options <= 10 => return Some(ReviewKey::Previous),
        _ => {}
    }
    match text {
        "/" => Some(ReviewKey::Search),
        "+" | "=" => Some(ReviewKey::ZoomIn),
        "-" => Some(ReviewKey::ZoomOut),
        _ => None,
    }
}

/// Maps a Ctrl chord to its physical form, for the screen-level shortcuts (see [`Chord`]).
pub fn page_chord(text: &str, ctrl: bool, alt: bool) -> Option<Chord> {
    if !ctrl || alt {
        return None;
    }
    if is_key(text, Key::Space) {
        return Some(Chord::Space);
    }
    physical_letter(text).map(|i| Chord::Letter((b'a' + i as u8) as char))
}

/// Maps a key press anywhere in the window to a global command.
pub fn global_key(text: &str, ctrl: bool, alt: bool) -> Option<GlobalKey> {
    if alt {
        return None;
    }
    if is_key(text, Key::Escape) {
        return Some(GlobalKey::Dismiss);
    }
    if !ctrl {
        return None;
    }
    if text == "," || text == "،" {
        return Some(GlobalKey::Settings);
    }
    match digit(text) {
        Some(0) => Some(GlobalKey::Settings),
        Some(d @ 1..=6) => Some(GlobalKey::Screen(d as u8 - 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(key: Key) -> String {
        key_char(key).to_string()
    }

    #[test]
    fn page_chords_use_physical_positions() {
        assert_eq!(page_chord("n", true, false), Some(Chord::Letter('n')));
        // Ctrl+N under the Arabic (101) layout produces ى on the N position
        assert_eq!(page_chord("ى", true, false), Some(Chord::Letter('n')));
        assert_eq!(page_chord(" ", true, false), Some(Chord::Space));
        assert_eq!(page_chord("n", false, false), None);
        assert_eq!(page_chord("n", true, true), None);
    }

    #[test]
    fn latin_letters_digits_and_arabic_select_the_same_positions() {
        for (i, latin) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            assert_eq!(option_for(latin, 5), Some(i as u8));
            assert_eq!(option_for(&latin.to_uppercase(), 5), Some(i as u8));
            assert_eq!(option_for(&(i + 1).to_string(), 5), Some(i as u8));
        }
        // typed Arabic option letters, abjad order
        for (i, ar) in ["أ", "ب", "ج", "د", "ه"].iter().enumerate() {
            assert_eq!(option_for(ar, 5), Some(i as u8), "{ar}");
        }
        assert_eq!(option_for("ا", 5), Some(0));
        // physical positions A..E under the Arabic (101) layout
        for (i, ar) in ["ش", "لا", "ؤ", "ي", "ث"].iter().enumerate() {
            assert_eq!(option_for(ar, 5), Some(i as u8), "{ar}");
        }
        assert_eq!(option_for("\u{FEFB}", 5), Some(1), "lam-alef ligature is B");
        // Arabic-Indic digits
        assert_eq!(option_for("٣", 5), Some(2));
        assert_eq!(option_for("۴", 5), Some(3));
        // out of range
        assert_eq!(option_for("f", 5), None);
        assert_eq!(option_for("6", 5), None);
        assert_eq!(option_for("0", 5), None);
        assert_eq!(option_for("و", 5), None);
    }

    #[test]
    fn layouts_never_disagree_up_to_five_options() {
        // No Arabic-101 character on positions A–E is also an abjad letter for A–E, and no
        // abjad letter for A–E sits on positions A–E: the two readings cannot conflict.
        for (pos, ar) in ARABIC_101.iter().enumerate() {
            let ab = ar
                .chars()
                .next()
                .filter(|_| ar.chars().count() == 1)
                .and_then(abjad_index);
            if let Some(ab) = ab
                && ab < 5
            {
                assert!(pos >= 5, "{ar} is abjad {ab} and sits on position {pos}");
            }
            if pos < 5 {
                assert!(ab.is_none_or(|ab| ab >= 5), "{ar} on position {pos}");
            }
        }
        // the documented overlaps on wider items: abjad wins
        assert_eq!(option_for("ب", 8), Some(1));
        assert_eq!(option_for("ا", 8), Some(0));
    }

    #[test]
    fn review_commands() {
        let m5 = KeyContext::Mark { options: 5 };
        let r = |t: &str, ctrl, shift| review_key(t, ctrl, shift, false, m5, true);
        assert_eq!(r(&k(Key::Return), false, false), Some(ReviewKey::Accept));
        assert_eq!(
            r(&k(Key::Return), false, true),
            Some(ReviewKey::ApplyToGroup)
        );
        assert_eq!(r(&k(Key::DownArrow), false, false), Some(ReviewKey::Next));
        assert_eq!(r(&k(Key::UpArrow), false, false), Some(ReviewKey::Previous));
        assert_eq!(r("j", false, false), Some(ReviewKey::Next));
        assert_eq!(r("k", false, false), Some(ReviewKey::Previous));
        assert_eq!(r(&k(Key::Backspace), false, false), Some(ReviewKey::Blank));
        assert_eq!(r("m", true, false), Some(ReviewKey::Multi));
        assert_eq!(
            r("ة", true, false),
            Some(ReviewKey::Multi),
            "Ctrl+M on Arabic layout"
        );
        assert_eq!(r("u", true, false), Some(ReviewKey::Unreadable));
        assert_eq!(r("d", true, false), Some(ReviewKey::Defer));
        assert_eq!(
            r("ي", true, false),
            Some(ReviewKey::Defer),
            "Ctrl+D on Arabic layout"
        );
        assert_eq!(r("e", true, false), Some(ReviewKey::Escalate));
        assert_eq!(r("r", true, false), Some(ReviewKey::Rescan));
        assert_eq!(r("z", true, false), Some(ReviewKey::Undo));
        assert_eq!(r("ئ", true, false), Some(ReviewKey::Undo));
        assert_eq!(r(" ", false, false), Some(ReviewKey::ToggleOverlay));
        assert_eq!(
            r(&k(Key::F2), false, false),
            Some(ReviewKey::ToggleOriginal)
        );
        assert_eq!(r(&k(Key::F1), false, false), Some(ReviewKey::Help));
        assert_eq!(r("/", false, false), Some(ReviewKey::Search));
        assert_eq!(r("b", false, false), Some(ReviewKey::Option(1)));
        assert_eq!(r("b", true, false), None, "Ctrl+B is not bound");
        assert_eq!(
            review_key(
                "b",
                false,
                false,
                true,
                KeyContext::Mark { options: 5 },
                true
            ),
            None,
            "no Alt bindings"
        );
    }

    #[test]
    fn single_key_switch_disables_character_keys_only() {
        let r = |t: &str, ctrl| {
            review_key(
                t,
                ctrl,
                false,
                false,
                KeyContext::Mark { options: 5 },
                false,
            )
        };
        assert_eq!(r("b", false), None);
        assert_eq!(r("/", false), None);
        assert_eq!(r(&k(Key::Return), false), Some(ReviewKey::Accept));
        assert_eq!(r("m", true), Some(ReviewKey::Multi));
        assert_eq!(r(&k(Key::DownArrow), false), Some(ReviewKey::Next));
    }

    #[test]
    fn j_and_k_belong_to_options_on_wide_items() {
        assert_eq!(
            review_key(
                "j",
                false,
                false,
                false,
                KeyContext::Mark { options: 10 },
                true
            ),
            Some(ReviewKey::Option(9))
        );
        assert_eq!(
            review_key(
                "k",
                false,
                false,
                false,
                KeyContext::Mark { options: 10 },
                true
            ),
            Some(ReviewKey::Previous)
        );
        assert_eq!(
            review_key(
                "k",
                false,
                false,
                false,
                KeyContext::Mark { options: 11 },
                true
            ),
            Some(ReviewKey::Option(10))
        );
    }

    #[test]
    fn identity_context_uses_digits_and_n_d() {
        let id = KeyContext::Identity { candidates: 3 };
        let r = |t: &str| review_key(t, false, false, false, id, true);
        assert_eq!(r("1"), Some(ReviewKey::Candidate(0)));
        assert_eq!(r("٣"), Some(ReviewKey::Candidate(2)));
        assert_eq!(r("4"), None);
        assert_eq!(r("n"), Some(ReviewKey::NotOnRoster));
        assert_eq!(
            r("ى"),
            Some(ReviewKey::NotOnRoster),
            "N on the Arabic layout"
        );
        assert_eq!(r("d"), Some(ReviewKey::Duplicate));
        assert_eq!(r("a"), None);
        assert_eq!(r("j"), Some(ReviewKey::Next));
        let notice = |t: &str| review_key(t, false, false, false, KeyContext::Notice, true);
        assert_eq!(notice("a"), None);
        assert_eq!(notice(&k(Key::Return)), Some(ReviewKey::Accept));
    }

    #[test]
    fn global_navigation() {
        assert_eq!(global_key("1", true, false), Some(GlobalKey::Screen(0)));
        assert_eq!(global_key("٤", true, false), Some(GlobalKey::Screen(3)));
        assert_eq!(global_key("6", true, false), Some(GlobalKey::Screen(5)));
        assert_eq!(global_key("7", true, false), None);
        assert_eq!(global_key(",", true, false), Some(GlobalKey::Settings));
        assert_eq!(global_key("1", false, false), None);
        assert_eq!(
            global_key(&k(Key::Escape), false, false),
            Some(GlobalKey::Dismiss)
        );
    }
}
