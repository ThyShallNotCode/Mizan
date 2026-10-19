//! Helpers that drive the shell headless through accessible labels and key events (A11).

#![allow(dead_code)]

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use shell_slint::ui::AppWindow;
use slint::ComponentHandle;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use std::collections::BTreeMap;

/// Distinct elements with exactly this accessible label. `find_all` reports an element once per
/// level of its inheritance chain, so duplicates are folded by geometry.
pub fn by_label(ui: &AppWindow, label: &str) -> Vec<ElementHandle> {
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
        "exactly one control labelled {label:?}, found {}",
        v.len()
    );
    v.into_iter().next().unwrap()
}

/// All distinct elements whose label starts with `prefix`.
pub fn with_prefix(ui: &AppWindow, prefix: &str) -> Vec<(String, ElementHandle)> {
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
    let p = e.absolute_position();
    let s = e.size();
    click_at(ui, p.x + s.width / 2.0, p.y + s.height / 2.0);
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
