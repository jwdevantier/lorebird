fn main() {
    // Compile GResource bundle (icons, etc.)
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/org.loreread.app.gresource.xml",
        "org.loreread.app.gresource",
    );

    // Embed icon into Windows .exe
    #[cfg(windows)]
    {
        let icon_path = std::path::Path::new("resources/org.loreread.app.ico");
        if !icon_path.exists() {
            panic!("Windows icon not found: {:?}", icon_path);
        }
        let mut res = winres::WindowsResource::new();
        res.set_icon(icon_path.to_str().unwrap());
        // Also set the application name for Windows metadata
        res.set("FileDescription", "loreread — Lightweight mail reader");
        res.set("ProductName", "loreread");
        res.set("OriginalFilename", "loreread.exe");
        res.compile().expect("Failed to compile Windows resources");
    }
}