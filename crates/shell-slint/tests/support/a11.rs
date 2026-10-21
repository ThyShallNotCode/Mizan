//! Helpers that drive the shell headless through accessible labels and key events (A11).

#![allow(dead_code)]

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use shell_slint::ui::AppWindow;
use slint::ComponentHandle;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use std::collections::BTreeMap;

/// Lets the window settle as Slint does before it draws a frame: the change handlers run and
/// what they make is made, with no time passing (only a timer already due fires). A user aims
/// at a drawn, settled frame, so a test finds and measures controls after it too. A pointer
/// event settles the window before it hits a control: a place read before would be one the
/// control may just have left, as the New-exam card's buttons move once a run's notice has
/// taken its room under them. Every query here settles first, so they all read one frame.
pub fn settle() {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
}

/// Distinct elements with exactly this accessible label. `find_all` reports an element once per
/// level of its inheritance chain, so duplicates are folded by geometry.
pub fn by_label(ui: &AppWindow, label: &str) -> Vec<ElementHandle> {
    settle();
    let mut seen = BTreeMap::new();
    for e in ElementHandle::find_by_accessible_label(ui, label) {
        let p = e.absolute_position();
        let s = e.size();
        let key = (
            (p.x * 4.0) as i64,
            (p.y * 4.0) as i64,
            (s.width * 4.0) as i64,
            (s.height * 4.0) as i64,
        );
        seen.entry(key).or_insert(e);
    }
    seen.into_values().collect()
}

/// The navigation tab with this label (a status pill may carry the same word).
pub fn tab(ui: &AppWindow, label: &str) -> ElementHandle {
    let v: Vec<_> = by_label(ui, label)
        .into_iter()
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Tab))
        .collect();
    assert_eq!(v.len(), 1, "exactly one tab labelled {label:?}");
    v.into_iter().next().unwrap()
}

pub fn one(ui: &AppWindow, label: &str) -> ElementHandle {
    let v = by_label(ui, label);
    assert_eq!(
        v.len(),
        1,
        "exactly one control labelled {label:?}, found {}; {}",
        v.len(),
        near(ui, label)
    );
    v.into_iter().next().unwrap()
}

/// Where things are, for a failure message: the window's size and every element whose label
/// shares a word with `label`, with its place.
pub fn near(ui: &AppWindow, label: &str) -> String {
    settle();
    let size = ui.window().size();
    let words: Vec<&str> = label.split_whitespace().filter(|w| w.len() > 3).collect();
    let mut found = Vec::new();
    for e in ElementQuery::from_root(ui).match_descendants().find_all() {
        if let Some(l) = e.accessible_label()
            && words.iter().any(|w| l.contains(w))
        {
            let (p, sz) = (e.absolute_position(), e.size());
            let item = format!(
                "{:?} at ({:.0}, {:.0}) {:.0}x{:.0}",
                l.as_str(),
                p.x,
                p.y,
                sz.width,
                sz.height
            );
            if !found.contains(&item) {
                found.push(item);
            }
        }
    }
    format!(
        "window {}x{}; near: [{}]",
        size.width,
        size.height,
        found.join("; ")
    )
}

/// All distinct elements whose label starts with `prefix`.
pub fn with_prefix(ui: &AppWindow, prefix: &str) -> Vec<(String, ElementHandle)> {
    settle();
    let mut out: BTreeMap<String, ElementHandle> = BTreeMap::new();
    for e in ElementQuery::from_root(ui).match_descendants().find_all() {
        if let Some(l) = e.accessible_label()
            && l.starts_with(prefix)
        {
            out.entry(l.to_string()).or_insert(e);
        }
    }
    out.into_iter().collect()
}

pub fn selected_queue_row(ui: &AppWindow) -> String {
    settle();
    let rows: Vec<String> = ElementQuery::from_root(ui)
        .match_descendants()
        .match_accessible_role(AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .filter(|e| e.accessible_item_selected() == Some(true))
        .filter_map(|e| e.accessible_label().map(|l| l.to_string()))
        .collect();
    rows.into_iter()
        .find(|l| l.starts_with('Q') || l.starts_with("Booklet"))
        .unwrap_or_default()
}

pub fn key(ui: &AppWindow, text: &str) {
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
}

pub fn special(ui: &AppWindow, k: Key) {
    key(ui, &char::from(k).to_string());
}

pub fn chord(ui: &AppWindow, modifier: Key, text: &str) {
    let m: String = char::from(modifier).to_string();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: m.clone().into(),
    });
    key(ui, text);
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: m.into() });
}

/// One left click at the centre of `e`, through the window's pointer events (move, press,
/// release) as a mouse delivers it, not through the accessibility action.
pub fn click(ui: &AppWindow, e: &ElementHandle) {
    settle();
    // a control the window removed as it settled would be clicked at (0, 0)
    assert!(
        e.is_valid() && e.size().width > 0.0 && e.size().height > 0.0,
        "{:?} went away before it was clicked",
        e.accessible_label().unwrap_or_default().as_str()
    );
    let p = e.absolute_position();
    let s = e.size();
    let (x, y) = (p.x + s.width / 2.0, p.y + s.height / 2.0);
    // a click past the window's edge reaches nothing, and the test would go on as if it had
    let size = ui.window().size();
    assert!(
        x >= 0.0 && y >= 0.0 && x < size.width as f32 && y < size.height as f32,
        "{:?} is clicked at ({x:.0}, {y:.0}), outside the {}x{} window",
        e.accessible_label().unwrap_or_default().as_str(),
        size.width,
        size.height
    );
    click_at(ui, x, y);
}

/// What lies under a window position, topmost last, for a failure message: every element
/// whose box holds the point, by its accessible label or else its type.
pub fn under(ui: &AppWindow, x: f32, y: f32) -> String {
    settle();
    let mut found = Vec::new();
    for e in ElementQuery::from_root(ui).match_descendants().find_all() {
        let (p, sz) = (e.absolute_position(), e.size());
        if x < p.x || y < p.y || x >= p.x + sz.width || y >= p.y + sz.height {
            continue;
        }
        let name = match e.accessible_label() {
            Some(l) if !l.is_empty() => format!("{:?}", l.as_str()),
            _ => e.type_name().unwrap_or_default().to_string(),
        };
        let item = format!(
            "{name} ({:.0}, {:.0}) {:.0}x{:.0}",
            p.x, p.y, sz.width, sz.height
        );
        if !found.contains(&item) {
            found.push(item);
        }
    }
    format!("under ({x:.0}, {y:.0}): [{}]", found.join("; "))
}

/// One left click at a window position (for a spot that is no control, such as an overlay's
/// background).
pub fn click_at(ui: &AppWindow, x: f32, y: f32) {
    press_at(ui, x, y);
    release_at(ui, x, y);
}

/// The first half of a click at a window position: the pointer moves there and presses (a
/// test may let the shell refresh before [`release_at`]).
pub fn press_at(ui: &AppWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let w = ui.window();
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
}

/// The second half of a click: the left button released at a window position.
pub fn release_at(ui: &AppWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: slint::LogicalPosition::new(x, y),
        button: PointerEventButton::Left,
    });
}

/// Types `text` one key at a time into whatever has focus, as a keyboard does.
pub fn type_text(ui: &AppWindow, text: &str) {
    for ch in text.chars() {
        key(ui, &ch.to_string());
    }
}

/// Every text the accessible tree of the visible elements carries (labels, values,
/// descriptions, placeholders): what a screen reader, or a test, reads off the screen.
pub fn all_texts(ui: &AppWindow) -> Vec<String> {
    settle();
    let mut out = Vec::new();
    for e in ElementQuery::from_root(ui).match_descendants().find_all() {
        let texts = [
            e.accessible_label(),
            e.accessible_value(),
            e.accessible_description(),
            e.accessible_placeholder_text(),
        ];
        out.extend(
            texts
                .into_iter()
                .flatten()
                .filter(|t| !t.is_empty())
                .map(|t| t.to_string()),
        );
    }
    out
}
