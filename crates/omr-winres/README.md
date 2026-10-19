# omr-winres

Build-time helper for `omr-desktop.exe` and `omr.exe`: embeds the Mizan icon (`assets/mizan.ico`,
16 to 256 px) and a version resource (file description, product name, version) into Windows
builds. It writes the resource file itself, so building needs no `rc.exe`, `windres` or other
crate:

- **MSVC:** a `.res` file, which `link.exe` and `lld-link` accept as an input;
- **GNU (mingw):** a COFF object with one `.rsrc` section, since GNU `ld` does not read `.res`.

On other targets it does nothing. Call it from a `build.rs`:

```rust
fn main() {
    omr_winres::embed(&omr_winres::Info {
        file_description: "Mizan",
        original_filename: "omr-desktop.exe",
    });
}
```

The icon is generated from the vector sources in `crates/shell-slint/ui/assets/` by
`cargo run -p shell-slint --example brand_icons`.
