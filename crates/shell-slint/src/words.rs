//! Text of the grading core in this window's words.
//!
//! The core words its refusals, findings and holds for the command line as well: they name
//! its commands (`omr key confirm`), its flags (`--forms`), the sections of the design and the
//! ids of its open decisions. The window shows none of these: it names its own controls by
//! their labels instead. [`window_words`] says each phrase the core writes for the command line
//! the way this window does it, and then drops what is left of the command line: a parenthesis
//! that names a command or a flag, a section number, a decision id. What the core quotes in its
//! texts (a file name, a path, a learning objective's name) is left as it is: only the core's
//! own shapes are taken out, and the spaces and punctuation are put right only where they were.

/// A phrase of the core and the window's words for it, tried in this order. In a phrase, `{}`
/// stands for one word (a version, a number, `5=B`), written again where the words have `{}`;
/// `{*}` stands for any text without a parenthesis, which the words leave out.
const PHRASES: &[(&str, &str)] = &[
    // ---- the key read from key sheets
    (
        "check the key summary (key_sheet_summary.html, or omr key summary) and confirm it \
         (omr key confirm)",
        "check every row in Setup (Key from answer sheets) and confirm it there",
    ),
    (
        "check every row of the key summary (key_sheet_summary.html, or omr key summary), \
         decide the highlighted rows, then confirm (omr key confirm)",
        "check every row in Setup (Key from answer sheets), decide the highlighted rows, then \
         confirm the key there",
    ),
    (
        "(see the key summary: rescan them, or give the key as a file with omr key replace)",
        "(see Key from answer sheets in Setup: rescan them, or give the key as a file with \
         Replace key…)",
    ),
    // The window cannot turn the search of key sheets in the stack off: it says what follows.
    (
        "grade again naming the key sheet (--key-sheet FILE --key-sheet-marker none) so that \
         the student is graded",
        "that student is not graded: Mizan cannot tell the sheet from a key sheet",
    ),
    (
        "grade again naming the real key sheet (--key-sheet FILE --key-sheet-marker none) so \
         that the student is graded",
        "that student is not graded: Mizan cannot tell the sheet from a key sheet",
    ),
    (
        "grade again naming the key sheets (--key-sheet FILE --key-sheet-marker none) so that \
         it is graded as a student",
        "it is not graded as a student: Mizan cannot tell it from a key sheet",
    ),
    ("; check the key summary", "; check the key in Setup"),
    (
        "(name key sheets with --key-sheet)",
        "(give the key sheets in the Key sheets field of New exam, or Read key from answer \
         sheet… in Setup)",
    ),
    // ---- the key
    (
        "(Setup › Replace key…, or omr key replace)",
        "(Setup › Replace key…)",
    ),
    ("(Replace key…, or omr key replace)", "(Replace key…)"),
    (" (omr review list --lane scoring)", " in Review"),
    (
        "; set each one (omr key set) or void it with *",
        "; type each one in the key grid in Setup, or * to void it",
    ),
    (
        ". Resolve the open items (omr review list) or supply the key (omr key replace), then \
         release again",
        ". Resolve the open items in Review, or replace the key in Setup (Replace key…), then \
         release again",
    ),
    ("(omr key replace --key key.csv)", "(Replace key… in Setup)"),
    (
        "no key yet (omr key set / omr key replace)",
        "no key yet (give one with Replace key…)",
    ),
    (
        "when you confirm them (omr key confirm --answer {}), or give the whole key as a file \
         (omr key replace)",
        "when you confirm them in Setup (Key from answer sheets), or give the whole key as a \
         file with Replace key…",
    ),
    (
        "when you confirm them (omr key confirm), or give the whole key as a file (omr key \
         replace)",
        "when you confirm them in Setup (Key from answer sheets), or give the whole key as a \
         file with Replace key…",
    ),
    (
        "grade the exam again with this key (omr grade --key)",
        "grade the scans again as a new exam with this key (New exam)",
    ),
    (
        "grade it again with this key (omr grade --key)",
        "grade the scans again as a new exam with this key (New exam)",
    ),
    (
        "; correct one entry with omr key set",
        "; correct one entry in the key grid in Setup",
    ),
    (
        "(change a confirmed key with omr key set)",
        "(change a confirmed key in the key grid in Setup)",
    ),
    (
        "; grade with `omr grade --key` or replace the key with `omr key replace --key`, which \
         read points per version",
        "; replace the key with Replace key… in Setup, which reads points per version",
    ),
    // ---- the versions, the questions and the options
    (
        "; grade with --forms including {}, or leave the sheet out",
        "; grade the scans with version {} among the exam's versions (Versions, on the New exam \
         card), or leave the sheet out",
    ),
    (
        "or declare the version (--forms)",
        "or declare the version (Versions, on the New exam card)",
    ),
    (
        "declare the version the key is for (--forms {})",
        "declare the version the key is for (Versions, on the New exam card)",
    ),
    (
        "or declare the exam's forms to match the file (--forms)",
        "or declare the exam's versions to match the file (Versions, on the New exam card)",
    ),
    (
        "declare every version the file has (--forms)",
        "declare every version the file has (Versions, on the New exam card)",
    ),
    (
        ": pass --id-scheme {}",
        ": type {} in Student ID scheme (Advanced options of New exam)",
    ),
    (
        "grade it again with --options set to them",
        "grade it again with Options per question set to them",
    ),
    (
        "grade with --options {}",
        "grade with Options per question set to {}",
    ),
    (
        "check that --questions, --options and --forms match",
        "check that the number of questions, the options per question and the versions match",
    ),
    (
        "then grade again into a new folder",
        "then grade the scans again with New exam",
    ),
    // ---- the sheet
    (
        "; give the spec file `omr sheet render` printed from",
        "; give the spec file the sheets were printed from",
    ),
    (
        "; `omr templates --example-spec` prints one",
        "; give the spec file the sheets were printed from",
    ),
    // ---- the CLO mapping
    (
        ", e.g. --approver \"Dr. A. Coordinator\"",
        ": type your name in Settings (Your name in the audit trail), then approve again",
    ),
    (
        "; give one with omr clo replace --clo FILE",
        "; give one with Replace CLOs in Setup",
    ),
    (
        "grade the exam again with --clo ",
        "grade the scans again as a new exam with the CLO mapping file ",
    ),
    (
        "change the rows in clo.csv (its `form` column names each row's version) and grade the \
         exam again with it (omr grade … --clo clo.csv)",
        "change the rows in a CLO file (its form column names each row's version) and give it \
         with Replace CLOs",
    ),
    // ---- the exam folder
    (
        "; run `omr grade … --out DIR` first, then point --project at DIR",
        "",
    ),
    (
        "; open it for writing (omr add --project DIR) to add scans",
        ": reopen it once no other program has it open, then add the scans",
    ),
    (
        "; supply them (omr key replace, --roster) and release first",
        "; give them in Setup (Replace key…, Add roster…), then release",
    ),
    // ---- review decisions
    (
        ": give a reason code (§13.4), e.g. {*} (CLI: --reason CODE [--note TEXT])",
        ": choose a reason for it",
    ),
    (
        "needs a reason code and a note saying why (§12.2), e.g. --reason ADJUDICATION --note \
         \"checked with the instructor: the key is right\"",
        "needs a reason and a note saying why: choose a reason and write the note",
    ),
    // ---- confirming the key read from key sheets
    (
        ". Decide each one with --answer QUESTION=ANSWER (B; A|C to accept both; * to void) \
         together with --version and confirm again",
        ": decide each one in Setup (Key from answer sheets), then confirm the key there",
    ),
    (
        ". Decide each one with --answer QUESTION=ANSWER (B; A|C to accept both; * to void) and \
         confirm again",
        ": decide each one in Setup (Key from answer sheets), then confirm the key there",
    ),
    // ---- the exam folder held by another program
    (
        "(an `omr` command or a desktop window)",
        "(another Mizan window, for example)",
    ),
    // ---- the parallel run with Remark
    (
        "To have each decided in Review before release (spec §23.7), queue them (omr compare \
         remark --queue; in the desktop app, Queue for review).",
        "To have each decided in Review before release, choose Queue for review.",
    ),
    ("(MRK-001 parallel_run_disagreement)", "(MRK-001)"),
    (
        "each decision is recorded as a field label (spec §23.7).",
        "each decision is recorded.",
    ),
    (
        "give the points with --points FILE",
        "give the points in a points file",
    ),
    ("with --points FILE", "with a points file"),
];

/// The commands of the command line (`omr key confirm`): a parenthesis that names one is the
/// command line's.
const COMMANDS: &[&str] = &[
    "add",
    "clo",
    "compare",
    "grade",
    "import",
    "key",
    "release",
    "review",
    "sheet",
    "survey",
    "templates",
];

/// The programs and crates of Mizan (`omr-app`, `omr-desktop`): a parenthesis that names one
/// is the command line's.
const PROGRAMS: &[&str] = &[
    "analytics",
    "app",
    "chain",
    "cli",
    "core",
    "desktop",
    "identity",
    "pdf",
    "project",
    "recog",
    "remark",
    "scoring",
    "sim",
    "store",
    "verify",
];

/// The ids of the open decisions the core's texts name (`(spec §3.5, Q-08)`): only these are
/// taken out, so a `(Q-01)` in a file name or a learning objective's name stays.
const DECISIONS: &[&str] = &["Q-08"];

/// What a `(decision …)` parenthesis of the core names after `decision `: a single option
/// letter (`decision C`), or one of these words (`decision ack --reason … --note …`).
const DECISION_WORDS: &[&str] = &["ack", "multi", "void"];

/// Where a removal took text out: [`tidy`] puts the spaces and the punctuation right there,
/// and nowhere else (a file name or a name the text quotes keeps its own).
const CUT: char = '\u{1}';

/// `s`, a text of the core, in this window's words (see the module).
pub fn window_words(s: &str) -> String {
    tidy(&undash_flags(&rewritten(s)))
}

/// `s` with the core's phrases said as this window's and its references and command-line
/// parentheses taken out: all of [`window_words`] but the last resort that says a flag left in
/// the open as words.
fn rewritten(s: &str) -> String {
    let mut t = s.to_string();
    for (from, to) in PHRASES {
        t = replace_phrase(&t, from, to);
    }
    let t = drop_references(&t).replace('`', "");
    drop_command_parentheses(&t)
}

/// The first technical reference in `s` (a section, a decision id, a command, a flag, a crate
/// or a code quote), or `None` when `s` names none: what the window never shows.
pub fn technical(s: &str) -> Option<&'static str> {
    if s.contains('§') {
        return Some("a section number (§)");
    }
    if s.contains('`') {
        return Some("a code quote (`)");
    }
    let b = s.as_bytes();
    for (i, _) in s.match_indices("Q-") {
        let digits = b
            .get(i + 2..i + 4)
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit));
        if digits && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            return Some("a decision id (Q-NN)");
        }
    }
    for (i, _) in s.match_indices("--") {
        if b.get(i + 2).is_some_and(u8::is_ascii_lowercase) && starts_word(s, i) {
            return Some("a command-line flag (--)");
        }
    }
    for w in ["omr ", "omr-", "omr_", "shell-slint", "shell_slint"] {
        if s.match_indices(w).any(|(i, _)| starts_word(s, i)) {
            return Some("a command or a crate (omr)");
        }
    }
    None
}

/// Whether a word of `s` starts at byte `i`: at the start, or after a space, a bracket, a quote
/// or a removal (never inside a word, a file name or a path).
fn starts_word(s: &str, i: usize) -> bool {
    i == 0
        || matches!(
            s.as_bytes()[i - 1],
            b' ' | b'(' | b'[' | b'"' | b'\n' | b'\x01'
        )
}

/// `s` with every `from` replaced by `to`, `{}` and `{*}` as [`PHRASES`] says (a phrase said as
/// nothing leaves a [`CUT`]).
fn replace_phrase(s: &str, from: &str, to: &str) -> String {
    let cut = CUT.to_string();
    let to = if to.is_empty() { cut.as_str() } else { to };
    if !from.contains('{') {
        return s.replace(from, to);
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some((start, end, words)) = find_phrase(rest, from) {
        out.push_str(&rest[..start]);
        let mut words = words.into_iter();
        let mut parts = to.split("{}");
        out.push_str(parts.next().unwrap_or(""));
        for p in parts {
            out.push_str(words.next().unwrap_or(""));
            out.push_str(p);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// The first place `from` (with `{}` and `{*}`) matches in `s`: its start, its end and the
/// words of its `{}`.
fn find_phrase<'a>(s: &'a str, from: &str) -> Option<(usize, usize, Vec<&'a str>)> {
    // literal parts, each after the wildcard that comes before it (none for the first)
    let mut parts: Vec<(Option<bool>, &str)> = Vec::new();
    let mut rest = from;
    let mut wild = None;
    while let Some(at) = rest.find('{') {
        let word = rest[at..].starts_with("{}");
        let any = rest[at..].starts_with("{*}");
        if !word && !any {
            break;
        }
        parts.push((wild, &rest[..at]));
        wild = Some(word);
        rest = &rest[at + if word { 2 } else { 3 }..];
    }
    parts.push((wild, rest));
    let (_, first) = parts[0];
    let mut from_at = 0;
    'start: while let Some(i) = s[from_at..].find(first) {
        let start = from_at + i;
        let mut at = start + first.len();
        let mut words = Vec::new();
        for &(wild, lit) in &parts[1..] {
            let tail = &s[at..];
            let taken = if wild == Some(true) {
                tail.char_indices()
                    .find(|(_, c)| !(c.is_alphanumeric() || ":=|*_".contains(*c)))
                    .map_or(tail.len(), |(n, _)| n)
            } else {
                match tail.find(lit) {
                    Some(n) if !tail[..n].contains(['(', ')']) => n,
                    _ => {
                        from_at = start + first.len().max(1);
                        continue 'start;
                    }
                }
            };
            if taken == 0 || !tail[taken..].starts_with(lit) {
                from_at = start + first.len().max(1);
                continue 'start;
            }
            if wild == Some(true) {
                words.push(&tail[..taken]);
            }
            at += taken + lit.len();
        }
        return Some((start, at, words));
    }
    None
}

/// `s` without its section numbers and decision ids, as the core writes them (`spec §3.5`,
/// `(§12.2)`, `, §9.9`, `(Q-08)`, `, Q-08)`), each with the comma or space before it.
fn drop_references(s: &str) -> String {
    let mut t = s.to_string();
    while let Some((mut start, end)) = next_reference(&t) {
        if t[..start].ends_with(", ") {
            start -= 2;
        } else if t[..start].ends_with(' ') {
            start -= 1;
        }
        t.replace_range(start..end, &CUT.to_string());
    }
    t
}

/// The first section number or decision id of `t` in a shape the core writes it: its start
/// and its end. A `§` elsewhere, or an id that is none of [`DECISIONS`] (in a name the text
/// quotes, `Quiz (Q-01).pdf`), is not one.
fn next_reference(t: &str) -> Option<(usize, usize)> {
    let b = t.as_bytes();
    let section = t.match_indices('§').find_map(|(i, _)| {
        let after = i + '§'.len_utf8();
        if !b.get(after).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        let start = if t[..i].ends_with("spec ") { i - 5 } else { i };
        let lead = &t[..start];
        let led = start < i
            || lead.is_empty()
            || ["(", ", ", "; ", "\u{1}"].iter().any(|p| lead.ends_with(p));
        if !led {
            return None;
        }
        let len = t[after..]
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '.'))
            .map_or(t.len() - after, |(n, _)| n);
        // a full stop that ends the sentence is not part of the number
        let len = if t[after..after + len].ends_with('.') {
            len - 1
        } else {
            len
        };
        Some((start, after + len))
    });
    let decision = t
        .match_indices("Q-")
        .map(|(i, _)| i)
        .find(|&i| {
            t.get(i..i + 4).is_some_and(|id| DECISIONS.contains(&id))
                && (t[..i].ends_with('(') || t[..i].ends_with(", "))
                && matches!(b.get(i + 4), Some(b')' | b','))
        })
        .map(|i| (i, i + 4));
    match (section, decision) {
        (Some(s), Some(d)) => Some(s.min(d)),
        (s, d) => s.or(d),
    }
}

/// `s` without the parentheses that name the command line ([`names_command`]) or a decision of
/// it (`(decision void)`), or that hold nothing any more, each with the space before it.
fn drop_command_parentheses(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let mut depth = 0;
        let close = rest[open..].char_indices().find_map(|(n, c)| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(open + n)
        });
        let Some(close) = close else {
            // a parenthesis never closed (a file name may hold one): what follows is still read
            out.push_str(&rest[..=open]);
            rest = &rest[open + 1..];
            continue;
        };
        let inside = &rest[open + 1..close];
        let drop = inside
            .trim_matches(|c: char| c.is_whitespace() || ",;:".contains(c) || c == CUT)
            .is_empty()
            || names_decision(inside)
            || names_command(inside);
        if drop {
            out.push_str(rest[..open].strip_suffix(' ').unwrap_or(&rest[..open]));
            out.push(CUT);
        } else {
            out.push_str(&rest[..=close]);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Whether `s`, the inside of a parenthesis, is the core's `(decision …)`: `decision` and then
/// one option letter (`decision C`) or one of [`DECISION_WORDS`], alone or before a flag
/// (`decision ack --reason … --note …`). A name that starts with the word (`(decision
/// making)`) is none.
fn names_decision(s: &str) -> bool {
    let Some(what) = s.strip_prefix("decision ") else {
        return false;
    };
    let (word, rest) = what.split_once(' ').unwrap_or((what, ""));
    let mut letters = word.chars();
    let letter = letters.next().is_some_and(char::is_alphabetic) && letters.next().is_none();
    (letter || DECISION_WORDS.contains(&word)) && (rest.is_empty() || rest.starts_with("--"))
}

/// Whether `s` names the command line: one of its commands (`omr key`), a flag (`--forms`,
/// [`flag_at`]), a program or crate (`omr-app`), the command line itself (`CLI:`) or a lint
/// rule (`lint L14`), each where a word starts: a file name or a path that holds `--` or `omr-`
/// names none.
fn names_command(s: &str) -> bool {
    let first_word = |rest: &str, list: &[&str]| {
        let w = rest
            .split(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .unwrap_or("");
        list.contains(&w)
    };
    let at_word = |mark: &str, then: &dyn Fn(&str) -> bool| {
        s.match_indices(mark)
            .any(|(i, _)| starts_word(s, i) && then(&s[i + mark.len()..]))
    };
    at_word("omr ", &|r| first_word(r, COMMANDS))
        || at_word("omr-", &|r| first_word(r, PROGRAMS))
        || s.match_indices("--").any(|(i, _)| flag_at(s, i).is_some())
        || at_word("CLI:", &|_| true)
        || at_word("lint L", &|r| r.starts_with(|c: char| c.is_ascii_digit()))
}

/// The length of the flag's name when a flag of the command line starts at byte `i` of `s`
/// (`--key-sheet`): a `--` where a word starts, then a lower-case letter. A `--` inside a word
/// (`midterm--v2.pdf`) is none, nor one that starts a file name or a path
/// (`midterm --final.pdf`, `--old/scan.pdf`): its word, without the punctuation that ends a
/// sentence or a parenthesis, holds a `.`, `/` or `\`.
fn flag_at(s: &str, i: usize) -> Option<usize> {
    let after = s.get(i + 2..)?;
    if !(s[i..].starts_with("--")
        && starts_word(s, i)
        && after.starts_with(|c: char| c.is_ascii_lowercase()))
    {
        return None;
    }
    let word = after
        .split(char::is_whitespace)
        .next()
        .unwrap_or("")
        .trim_end_matches(['.', ',', ';', ':', ')', ']', '"', '\'']);
    if word.contains(['.', '/', '\\']) {
        return None;
    }
    Some(
        after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(after.len()),
    )
}

/// `s` with a flag left in the open ([`flag_at`]) said as words: `--key-sheet` is `key sheet`.
/// The last resort for a flag no phrase knows (every text the tests know is said by a phrase).
fn undash_flags(s: &str) -> String {
    let mut out = String::new();
    let (mut from, mut search) = (0, 0);
    while let Some(n) = s[search..].find("--") {
        let i = search + n;
        let Some(len) = flag_at(s, i) else {
            search = i + 2;
            continue;
        };
        out.push_str(&s[from..i]);
        out.push_str(&s[i + 2..i + 2 + len].replace('-', " "));
        from = i + 2 + len;
        search = from;
    }
    out.push_str(&s[from..]);
    out
}

/// `s` with the spaces and the punctuation each removal ([`CUT`]) left behind put right: the
/// strongest punctuation on either side stays once, followed by a space, none at all next to
/// a bracket, a full stop or an end. Elsewhere `s` is left as it is.
fn tidy(s: &str) -> String {
    if !s.contains(CUT) {
        return s.to_string();
    }
    let gap = [' ', ',', ';', CUT];
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(CUT) {
        let before = &rest[..i];
        let kept = before.trim_end_matches(gap);
        let after = rest[i..].trim_start_matches(gap);
        let taken = format!(
            "{}{}",
            &before[kept.len()..],
            &rest[i..rest.len() - after.len()]
        );
        out.push_str(kept);
        let edge = matches!(out.chars().next_back(), None | Some('('))
            || matches!(after.chars().next(), None | Some(')' | '.'));
        if !edge {
            if taken.contains(';') {
                out.push_str("; ");
            } else if taken.contains(',') {
                out.push_str(", ");
            } else if taken.contains(' ') {
                out.push(' ');
            }
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texts the core writes for the command line that reach this window (the release gate,
    /// the key-sheet hold and refusals, the key and CLO refusals, the parallel run, the New-exam
    /// card), each as the core words it.
    const CORE: &[&str] = &[
        "the key read from the key sheets is not confirmed (SCO-003 BLOCKER): check the key \
         summary (key_sheet_summary.html, or omr key summary) and confirm it (omr key confirm); \
         nothing is scored until then",
        "key INFERRED from the sheets (SCO-003): not releasable until the real key replaces it \
         (Setup › Replace key…, or omr key replace)",
        "3 ERROR/BLOCKER open, 1 of them a probable mis-key (SCO-001): change the key, accept a \
         second answer, void the item, or keep the key with a reason and a note (omr review \
         list --lane scoring)",
        "key incomplete (SCO-003): no key for A-Q3, B-Q7; set each one (omr key set) or void it \
         with *",
        "release refused: 2 ERROR/BLOCKER open. Resolve the open items (omr review list) or \
         supply the key (omr key replace), then release again",
        "KEY SHEETS NOT CONFIRMED: version A read, to confirm; the 2 key sheets of version A \
         disagree on 1 question(s): if one of them is a student's sheet bubbled with the \
         marker, grade again naming the key sheet (--key-sheet FILE --key-sheet-marker none) so \
         that the student is graded; 1 key sheet(s) not usable (see the key summary: rescan \
         them, or give the key as a file with omr key replace). Key-sheet answers are never \
         accepted automatically: check every row of the key summary (key_sheet_summary.html, \
         or omr key summary), decide the highlighted rows, then confirm (omr key confirm). \
         Nothing is scored until then",
        "version A is given by 2 key sheets: a.pdf (named); b.pdf (found in the stack). Check \
         that each one is a key sheet: a student who bubbled the marker number and answered \
         like the key reads the same, and would not be graded. If one is a student's sheet, \
         grade again naming the real key sheet (--key-sheet FILE --key-sheet-marker none) so \
         that the student is graded",
        "key.pdf page 1 could not be read (REG-003 no page frame); rescan the key sheet, or \
         give the key as a file (omr key replace --key key.csv)",
        "key.pdf page 1: the key sheet's version is not bubbled (or not read clearly); bubble \
         the version (A, B …) on the key sheet and rescan it, or give the key as a file (omr \
         key replace --key key.csv). If this page is a student's sheet bubbled with the \
         marker, grade again naming the key sheets (--key-sheet FILE --key-sheet-marker none) \
         so that it is graded as a student",
        "key.pdf page 2: the key sheet is for version C, which this exam does not use (A, B); \
         grade with --forms including C, or leave the sheet out",
        "no key yet (omr key set / omr key replace)",
        "key-sheet marker 0000000000 is not used in the stack: a roster student has this number \
         (name key sheets with --key-sheet)",
        "the key comes from key sheets that are not confirmed yet: decide question 5 when you \
         confirm them (omr key confirm --answer 5=B), or give the whole key as a file (omr key \
         replace)",
        "the exam has no key yet; supply the whole key first (omr key replace --key key.csv)",
        "the new key gives the forms different answers or points, but the exam was graded with \
         one item set shared by every form; grade the exam again with this key (omr grade \
         --key)",
        "the key from the key sheets is already confirmed (or a key file replaced it); correct \
         one entry with omr key set",
        "`A+C` is an AND key (Remark's partial-credit syntax: all of these answers required, \
         spec §10.1), which this version does not score yet; write `A|C` if either answer \
         earns the question, or `*` to void it",
        "key file: a points_C column for version C, which the exam does not declare (forms A, \
         B); rename the column or declare the version (--forms)",
        "the key file has a key for version B only; this exam declares versions A, B: declare \
         the version the key is for (--forms B), or give a key for every declared version",
        "the sheet's student-ID grid has 10 digits but the ID scheme Ksu has 9: pass \
         --id-scheme iau",
        "the sheet's student-ID grid has 8 digits but the ID scheme Iau has 10: pass \
         --id-scheme digits:8",
        "SCO-027 version A: key letter distribution unlikely (p = 0.004); check the key \
         summary; one option is never keyed: if the exam uses 4 options, grade with --options 4",
        "score run refused; check that --questions, --options and --forms match the printed \
         sheet and the key, then grade again into a new folder",
        "INFERRED KEY: no answer key was supplied; each form's key is the modal confident \
         response per question. Results are provisional and not releasable while the key is \
         inferred: replace the key in Setup (Replace key…, or omr key replace).",
        "sheet.json: not JSON (expected value); give the spec file `omr sheet render` printed \
         from",
        "sheet.json: not a sheet spec (missing field); `omr templates --example-spec` prints one",
        "sheet.json: the sheet spec sets no cancel_mode, so the marking convention printed on \
         the sheets is unknown (lint L14); set `cancel_mode` in the spec to the one the sheets \
         print (erase_only, cross_cancels or fill_cancels_check)",
        "name the approver (the course coordinator or the quality officer who reviewed the \
         mapping), e.g. --approver \"Dr. A. Coordinator\"",
        "the exam has no CLO mapping to approve; give one with omr clo replace --clo FILE",
        "clo.csv maps questions of one version only (the form column), and this exam scores its \
         versions as one item set: grade the exam again with --clo clo.csv, or map every version \
         alike (leave the form column empty)",
        "this exam's CLO mappings differ between versions (rows per version, e.g. imported from \
         Remark): a cell of the grid stands for several rows, so it is not edited here; change \
         the rows in clo.csv (its `form` column names each row's version) and grade the exam \
         again with it (omr grade … --clo clo.csv)",
        "C:/exams/cs211: no graded exam here; run `omr grade … --out DIR` first, then point \
         --project at DIR",
        "this exam was opened read-only; open it for writing (omr add --project DIR) to add \
         scans",
        "the key read from the key sheets is not confirmed: confirm it first (omr key confirm), \
         or give the key as a file (omr key replace), then add the scans",
        "no Blackboard file: grades are pushed only when released (omr release), with a \
         supplied key (not inferred), a roster and a score run; supply them (omr key replace, \
         --roster) and release first",
        "B overrides the engine's proposal (C): give a reason code (§13.4), e.g. KEY_ERROR, \
         OTHER (CLI: --reason CODE [--note TEXT])",
        "keeping the key of item 12, a probable mis-key (SCO-001), needs a reason code and a \
         note saying why (§12.2), e.g. --reason ADJUDICATION --note \"checked with the \
         instructor: the key is right\"",
        "SCO-001 Probable mis-key: decide it: change the key to C (decision C), accept either \
         the key or C (decision multi), void the item (decision void), or keep the key with a \
         reason code and a note (decision ack --reason … --note …); flags never re-key on \
         their own, and a key change is an audited key version",
        "option E is marked, but the exam uses options A–D (--options 4): choose the answer \
         among A–D, or blank for no valid answer (scored as no answer); if the exam really has \
         more options, grade it again with --options set to them",
        "RESTRICTED (مقيد) - parallel run with Remark Office OMR: student numbers and answers \
         (PDPL personal data). Head-to-head figures stay inside the university and leave it \
         only after the licence review (spec §3.5, Q-08)",
        "This report does not hold release for its 2 settled difference(s) from an answer this \
         exam auto-accepted: they are not in the review queue. To have each decided in Review \
         before release (spec §23.7), queue them (omr compare remark --queue; in the desktop \
         app, Queue for review). 3 parallel-run difference(s) wait in Review (MRK-001 \
         parallel_run_disagreement), 3 queued from this comparison and hold release until the \
         instructor decides each against the bubble row; each decision is recorded as a field \
         label (spec §23.7).",
        "the licence review of parallel runs with Remark (Q-08) recorded by Legal on 2026-11-01 \
         does not permit them: no comparison is made; ask the university's legal affairs, or \
         remove licence_review.json from the private key folder if the decision was recorded in \
         error",
        "SYS-012 project is locked by another writer: C:/exams/cs211/project.omrproj; another \
         program has the exam open for writing (an `omr` command or a desktop window): close it \
         and try again",
        "the key cannot be confirmed yet: 2 row(s) need your decision: version A question 5: \
         the key sheets disagree (--answer 5=…); version B question 7: no answer read (--answer \
         7=…). Decide each one with --answer QUESTION=ANSWER (B; A|C to accept both; * to void) \
         together with --version and confirm again",
        "the key cannot be confirmed yet: 1 row(s) need your decision: version A question 5: \
         the key sheets disagree (--answer 5=…). Decide each one with --answer QUESTION=ANSWER \
         (B; A|C to accept both; * to void) and confirm again",
        "version A: the points (exam) do not reproduce 3 Remark total(s) exactly: rows 4, 9; \
         give the points with --points FILE, or check the grading in Remark and export again",
        "refusing to print a national ID or Iqama number on a sheet (§6.7)",
        "SQLite 3.30 is below the required 3.45 (WAL-reset bug, §14.2)",
    ];

    #[test]
    fn no_core_text_reaches_the_window_with_a_command_or_a_section() {
        for c in CORE {
            assert!(technical(c).is_some(), "the core text names nothing: {c}");
            let w = window_words(c);
            assert_eq!(technical(&w), None, "{w}\n  from {c}");
            assert!(
                !w.contains("  ") && !w.contains("()") && !w.contains(" ,"),
                "{w}"
            );
            // each is said by a phrase or a removal: the last resort that says a flag left in
            // the open as words (which no test could tell from plain words) is never needed
            let r = rewritten(c);
            assert_eq!(undash_flags(&r), r, "a flag no phrase says, from {c}");
        }
    }

    /// What a core text quotes (a file name, a path, a name) is left as it is, even beside
    /// what is taken out: only the core's own shapes are removed.
    #[test]
    fn the_names_a_text_quotes_are_left_as_they_are() {
        let says = |core: &str, window: &str| {
            assert_eq!(window_words(core), window, "from {core}");
        };
        says(
            "key--final.pdf page 1 could not be read (REG-003 no page frame); rescan the key \
             sheet, or give the key as a file (omr key replace --key key.csv)",
            "key--final.pdf page 1 could not be read (REG-003 no page frame); rescan the key \
             sheet, or give the key as a file (Replace key… in Setup)",
        );
        says(
            "C:/scans/Ali  Hassan,,1.pdf: no page could be read (omr survey); check the folder",
            "C:/scans/Ali  Hassan,,1.pdf: no page could be read; check the folder",
        );
        says(
            "scan (1.pdf: rescan it, or replace the key (omr key replace)",
            "scan (1.pdf: rescan it, or replace the key",
        );
        // a decision id, a decision or a flag in the shape a name or a path can take
        says(
            "mapping to unknown CLO `LO2 (Q-12)`",
            "mapping to unknown CLO LO2 (Q-12)",
        );
        says(
            "Quiz (Q-01).pdf page 1 could not be read (REG-003 no page frame); rescan the key \
             sheet, or give the key as a file (omr key replace --key key.csv)",
            "Quiz (Q-01).pdf page 1 could not be read (REG-003 no page frame); rescan the key \
             sheet, or give the key as a file (Replace key… in Setup)",
        );
        for t in [
            "CS211 midterm--section2.pdf p.3",
            "C:/scans/midterm--v2.pdf: cannot read it; check the path of the file",
            "Quiz Q-01 (sec 2).pdf p.3",
            "CS211 (omr batch).pdf p.2",
            "Booklet 0057 (scan--006.jpg page 1)",
            "Booklet 0057 (omr-scans/scan-006.jpg page 1)",
            "no page could be read (see C:/Users/a/omr-scans/day2); check the folder",
            "2 key sheets: Q-12 key.pdf (named)",
            "CS211--final.xlsx: an objective (Q-12 retake) with no question",
            "Knowledge §2.3: 4 questions",
            "Ali  Hassan, 1,,3",
            "Quiz (Q-01).xlsx: no version column filled; taken as version A (same answer key)",
            "group LO2 (Q-12) has 3 possible points, Q-12) of version A",
            "objective 'Ethics (decision making)' has no question",
            "objective 'Ethics (decision B making)' has no question",
            "C:/scans/midterm --final.pdf: cannot read it; check the path of the file",
            "Booklet 0057 (midterm --final.pdf page 1)",
            "no page could be read (see D:\\exams --old\\day2); check the folder",
        ] {
            assert_eq!(window_words(t), t);
        }
    }

    /// The decision ids the window takes out are the only ones a text of the core can name:
    /// the sources of the crates under the window name no other outside their comments.
    #[test]
    fn the_core_names_no_decision_id_the_window_keeps() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&crates)
            .expect("the crates folder")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| !p.ends_with("shell-slint"))
            .map(|p| p.join("src"))
            .filter(|p| p.is_dir())
            .collect();
        assert!(dirs.len() > 5, "the core's crates: {dirs:?}");
        let mut named = Vec::new();
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).expect("a source folder").flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).expect("a source file");
                for line in src.lines().filter(|l| !l.trim_start().starts_with("//")) {
                    for (i, _) in line.match_indices("Q-") {
                        if let Some(id) = line.get(i..i + 4)
                            && id.as_bytes()[2..].iter().all(u8::is_ascii_digit)
                            && (i == 0 || !line.as_bytes()[i - 1].is_ascii_alphanumeric())
                            && !DECISIONS.contains(&id)
                        {
                            named.push(format!("{}: {line}", p.display()));
                        }
                    }
                }
            }
        }
        assert!(named.is_empty(), "{named:#?}");
    }

    #[test]
    fn the_window_names_its_own_controls() {
        let says = |core: &str, window: &str| {
            assert_eq!(window_words(core), window, "from {core}");
        };
        says(
            "key INFERRED from the sheets (SCO-003): not releasable until the real key replaces \
             it (Setup › Replace key…, or omr key replace)",
            "key INFERRED from the sheets (SCO-003): not releasable until the real key replaces \
             it (Setup › Replace key…)",
        );
        says(
            "1 ERROR/BLOCKER open, 1 of them a probable mis-key (SCO-001): keep the key with a \
             reason and a note (omr review list --lane scoring)",
            "1 ERROR/BLOCKER open, 1 of them a probable mis-key (SCO-001): keep the key with a \
             reason and a note in Review",
        );
        says(
            "the key sheet is for version C, which this exam does not use (A, B); grade with \
             --forms including C, or leave the sheet out",
            "the key sheet is for version C, which this exam does not use (A, B); grade the \
             scans with version C among the exam's versions (Versions, on the New exam card), \
             or leave the sheet out",
        );
        says(
            "decide question 5 when you confirm them (omr key confirm --answer 5=A|C), or give \
             the whole key as a file (omr key replace)",
            "decide question 5 when you confirm them in Setup (Key from answer sheets), or give \
             the whole key as a file with Replace key…",
        );
        says(
            "the ID scheme Iau has 10: pass --id-scheme digits:8",
            "the ID scheme Iau has 10: type digits:8 in Student ID scheme (Advanced options of \
             New exam)",
        );
        says(
            "Head-to-head figures stay inside the university and leave it only after the \
             licence review (spec §3.5, Q-08)",
            "Head-to-head figures stay inside the university and leave it only after the \
             licence review",
        );
        says(
            "`+` is an AND key (all of these answers required, spec §10.1), which this version \
             does not score yet",
            "+ is an AND key (all of these answers required), which this version does not score \
             yet",
        );
        says(
            "decide it: change the key to C (decision C), void the item (decision void), or keep \
             the key (decision ack --reason … --note …); flags never re-key",
            "decide it: change the key to C, void the item, or keep the key; flags never re-key",
        );
        says(
            "C:/exams/cs211: no graded exam here; run `omr grade … --out DIR` first, then point \
             --project at DIR",
            "C:/exams/cs211: no graded exam here",
        );
        says(
            "B overrides the engine's proposal (C): give a reason code (§13.4), e.g. KEY_ERROR, \
             OTHER (CLI: --reason CODE [--note TEXT])",
            "B overrides the engine's proposal (C): choose a reason for it",
        );
        // a flag the phrases do not know is said as words, never shown as typed
        says("unknown --cancel-mode value", "unknown cancel mode value");
        says("use --cancel-mode.", "use cancel mode.");
        says(
            "Licence review (Q-08): not recorded, so open",
            "Licence review: not recorded, so open",
        );
    }

    #[test]
    fn plain_text_is_left_as_it_is() {
        for t in [
            "Key version 2 in force: the exam was rescored",
            "scans (1).pdf: 3 pages; version A (2 booklets)",
            "project.omr is not an OMR marker",
            "Q12 of version A-Q3 · TP-154 · SCO-003 · page 3--4",
        ] {
            assert_eq!(window_words(t), t);
        }
        assert_eq!(technical("project.omr is not an OMR marker"), None);
        assert_eq!(technical("Remark Office OMR · A-Q12 · SCO-003"), None);
        assert!(technical("open it with omr-desktop --workspace").is_some());
        assert!(technical("Q-08").is_some());
    }
}
