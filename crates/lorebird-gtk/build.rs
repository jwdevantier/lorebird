fn main() {
    // Compile GResource bundle (icons, etc.)
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/org.lorebird.app.gresource.xml",
        "org.lorebird.app.gresource",
    );

    // Embed icon into Windows .exe
    #[cfg(windows)]
    {
        let icon_path = std::path::Path::new("resources/org.lorebird.app.ico");
        if !icon_path.exists() {
            panic!("Windows icon not found: {:?}", icon_path);
        }
        let mut res = winres::WindowsResource::new();
        res.set_icon(icon_path.to_str().unwrap());
        // Also set the application name for Windows metadata
        res.set("FileDescription", "lorebird — Lightweight mail reader");
        res.set("ProductName", "lorebird");
        res.set("OriginalFilename", "lorebird.exe");
        res.compile().expect("Failed to compile Windows resources");
    }
}