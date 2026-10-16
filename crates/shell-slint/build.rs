fn main() {
    // Debug info gives every element its id and lets the testing backend's ElementHandle API
    // (find_by_accessible_label, find_by_element_id) see the tree: ADR-001 S7, A11.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_debug_info(true);
    slint_build::compile_with_config("ui/app.slint", config).expect("compile ui/app.slint");
}
