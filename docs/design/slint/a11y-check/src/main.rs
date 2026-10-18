//! Accessibility check for `crates/shell-slint/ui/theme.slint` (DESIGN.md §8, §10, §11).
//! Each assertion pins a property the A11 review-flow test relies on: one AT node per
//! control label, state exposed (`enabled`, `item-selected`, `expanded`), and accessible
//! actions that respect `enabled`.
//!
//! No `#![forbid(unsafe_code)]`: Slint 1.18 generated code expands `allow(unsafe_code)`
//! (E0453); this crate is outside the workspace, like ../harness.
fn main() {}

#[cfg(test)]
mod tests {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
    use slint::ComponentHandle;
    slint::include_modules!();

    fn one(ui: &Check, label: &str) -> ElementHandle {
        let v: Vec<_> = ElementHandle::find_by_accessible_label(ui, label).collect();
        assert_eq!(
            v.len(),
            1,
            "exactly one AT node labelled {label:?}, got {}",
            v.len()
        );
        v.into_iter().next().unwrap()
    }

    #[test]
    fn theme_components_expose_correct_accessibility() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = Check::new().unwrap();
        ui.global::<Theme>().set_dark(true);
        ui.global::<Theme>().set_reduce_motion(true);

        // NavItem: tab with selection state, not "checked"
        let review = one(&ui, "Review, 38 open");
        assert_eq!(review.accessible_role(), Some(AccessibleRole::Tab));
        assert_eq!(review.accessible_item_selected(), Some(true));
        assert_eq!(one(&ui, "Exams").accessible_item_selected(), Some(false));

        // SearchField: exactly one text-input node, labelled; set-value reaches `edited`
        let inputs = ElementQuery::from_root(&ui)
            .match_descendants()
            .match_accessible_role(AccessibleRole::TextInput)
            .find_all();
        // The query reports the SearchField item once per element in its inheritance
        // chain; what matters is that every text-input node is the SearchField itself.
        assert!(!inputs.is_empty());
        assert!(
            inputs
                .iter()
                .all(|e| e.type_name().as_deref() == Some("SearchField")),
            "the inner TextInput must not be a second text-input node"
        );
        let inner = ElementQuery::from_root(&ui)
            .match_descendants()
            .match_type_name("TextInput")
            .find_all();
        assert!(!inner.is_empty());
        assert!(
            inner
                .iter()
                .all(|e| e.accessible_role() == Some(AccessibleRole::None))
        );
        let search = one(&ui, "Roster search");
        assert_eq!(
            search.accessible_placeholder_text().as_deref(),
            Some("Roster search")
        );
        search.set_accessible_value("Al-Harbi");
        assert_eq!(ui.get_search_edited(), "Al-Harbi");
        assert_eq!(search.accessible_value().as_deref(), Some("Al-Harbi"));

        // Disabled IconButton / Button: state exposed, default action inert
        let filter = one(&ui, "Filter");
        assert_eq!(filter.accessible_enabled(), Some(false));
        filter.invoke_accessible_default_action();
        assert_eq!(ui.get_icon_clicks(), 0);
        let apply = one(&ui, "Apply to group");
        assert_eq!(apply.accessible_enabled(), Some(false));
        apply.invoke_accessible_default_action();
        assert_eq!(ui.get_button_clicks(), 0);
        let accept = one(&ui, "Accept suggestion");
        assert_eq!(accept.accessible_description().as_deref(), Some("Enter"));
        accept.invoke_accessible_default_action();
        assert_eq!(ui.get_button_clicks(), 10);

        // GroupHeader: expandable, state follows, default action toggles
        let grp = one(&ui, "MRK-001 Low-confidence mark, 41 items");
        assert_eq!(grp.accessible_expandable(), Some(true));
        assert_eq!(grp.accessible_expanded(), Some(false));
        grp.invoke_accessible_default_action();
        assert!(ui.get_group_expanded());
        assert_eq!(grp.accessible_expanded(), Some(true));

        // TableRow: selected (not checked), default action = clicked
        let row = one(&ui, "Q17 form B");
        assert_eq!(row.accessible_item_selected(), Some(true));
        assert_eq!(row.accessible_checked(), None, "no checkbox state on a row");
        row.invoke_accessible_default_action();
        assert_eq!(ui.get_row_clicks(), 1);

        // Single combined labels (no duplicate child text nodes)
        one(&ui, "Open exceptions: 147, 38 ERROR");
        one(&ui, "In review");
        one(&ui, "B, suggested");
        one(&ui, "D: ambiguous");
        assert_eq!(
            ElementHandle::find_by_accessible_label(&ui, "/").count(),
            0,
            "breadcrumb separators hidden"
        );
    }
}
