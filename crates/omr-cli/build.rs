fn main() {
    // Windows: the app icon and version resource in omr.exe.
    omr_winres::embed(&omr_winres::Info {
        file_description: "Mizan command line",
        original_filename: "omr.exe",
    });
}
