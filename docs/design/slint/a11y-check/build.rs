fn main() {
    // Default resource embedding, as the shell's build.rs. The ElementHandle API needs the
    // compiler's debug info (`with_debug_info`, or SLINT_EMIT_DEBUG_INFO=1).
    let cfg = slint_build::CompilerConfiguration::new()
        .with_include_paths(vec!["../../../../crates/shell-slint/ui".into()])
        .with_debug_info(true);
    slint_build::compile_with_config("ui/check.slint", cfg).expect("compile ui/check.slint");
}
