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
        let mut res = winres::WindowsResource::new();
        res.set_icon("resources/org.loreread.app.ico");
        res.compile().expect("Failed to compile Windows resources");
    }
}