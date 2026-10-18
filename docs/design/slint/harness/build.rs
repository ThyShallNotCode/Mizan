fn main() {
    // Embed textures so the software renderer needs no runtime image decoders.
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("../gallery.slint", config).expect("compile ../gallery.slint");
}
