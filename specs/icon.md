# Icon Integration for lorebird

This document describes how the application icon (`icon/lorebird.png`, 1024×1024 RGBA) is
made available at every level: the running GTK window, the Linux desktop launcher, and
the Windows executable.

---

## 1. Source Artwork

```
icon/lorebird.png          ← original 1024×1024 RGBA PNG
icon/org.lorebird.app.png  ← 256×256 convenience copy
```

From the single 1024×1024 master, standard sizes are generated with ImageMagick:

```bash
for s in 16 32 48 64 128 256; do
    magick lorebird.png -resize ${s}x${s} org.lorebird.app.${s}.png
done
magick lorebird.png -define icon:auto-resize=16,32,48,64,128,256 org.lorebird.app.ico
```

Resulting files:

| File | Purpose |
|------|---------|
| `org.lorebird.app.16.png` | 16×16 icon |
| `org.lorebird.app.32.png` | 32×32 icon |
| `org.lorebird.app.48.png` | 48×48 icon |
| `org.lorebird.app.64.png` | 64×64 icon |
| `org.lorebird.app.128.png` | 128×128 icon |
| `org.lorebird.app.256.png` | 256×256 icon |
| `org.lorebird.app.ico` | Windows multi-size ICO (all sizes above) |

---

## 2. GResource Bundle (compiled into the binary)

GTK applications ship resources (icons, UI definitions, etc.) in a **GResource bundle** — a
binary blob that gets compiled into the executable at link time. This means the icon is
available without any filesystem install, even on a bare `$HOME`.

### 2.1 Resource directory layout

```
crates/lorebird-gtk/resources/
├── org.lorebird.app.gresource.xml   ← manifest
├── org.lorebird.app.16.png
├── org.lorebird.app.32.png
├── org.lorebird.app.48.png
├── org.lorebird.app.64.png
├── org.lorebird.app.128.png
├── org.lorebird.app.256.png
└── org.lorebird.app.ico              ← not in the bundle, Windows-only
```

### 2.2 GResource XML manifest

`resources/org.lorebird.app.gresource.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<gresources>
  <gresource prefix="/org/lorebird/app">
    <file>org.lorebird.app.16.png</file>
    <file>org.lorebird.app.32.png</file>
    <file>org.lorebird.app.48.png</file>
    <file>org.lorebird.app.64.png</file>
    <file>org.lorebird.app.128.png</file>
    <file>org.lorebird.app.256.png</file>
  </gresource>
  <gresource prefix="/org/lorebird/app/icons/scalable/apps">
    <file alias="org.lorebird.app.png">org.lorebird.app.256.png</file>
  </gresource>
</gresources>
```

Two gresource prefixes:

- **`/org/lorebird/app`** — the individual sizes, accessible programmatically.
- **`/org/lorebird/app/icons/scalable/apps`** — this follows the [icon theme
  specification](https://specifications.freedesktop.org/icon-theme-spec/icon-theme-spec-latest.html)
  directory layout. When we add this resource path to the GTK icon theme, GTK's
  `IconTheme::lookup_icon("org.lorebird.app")` can find it as if it were a file at
  `/usr/share/icons/hicolor/scalable/apps/org.lorebird.app.png`. This is what makes
  `.icon_name("org.lorebird.app")` on a window work.

### 2.3 Build script (`build.rs`)

```rust
fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/org.lorebird.app.gresource.xml",
        "org.lorebird.app.gresource",
    );

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("resources/org.lorebird.app.ico");
        res.compile().expect("Failed to compile Windows resources");
    }
}
```

`glib_build_tools::compile_resources()` runs `glib-compile-resources` at build time
(it must be on `$PATH` — the Nix dev shell provides it via the `glib` package).
The output is a `.c` file containing a byte array with the entire resource blob, which
gets linked into the binary.

The `#[cfg(windows)]` block uses the `winres` crate to embed a `.ico` into the final
`.exe`. This is a link-time operation — `windres` (or `rc.exe` on MSVC) produces a `.res`
object that the linker includes.

### 2.4 Cargo.toml additions

```toml
[build-dependencies]
glib-build-tools = "0.22"

[target.'cfg(windows)'.build-dependencies]
winres = "0.1"
```

**Version alignment matters**: `glib-build-tools` must match the `gio`/`glib` version used at
runtime. Our runtime `gio` is v0.22, so we use `glib-build-tools` v0.22. A mismatch
produces a "incompatible resource format" panic at startup.

---

## 3. Registering at Runtime

In `main.rs`, before creating the GTK application:

```rust
fn main() {
    // 1. Load the compiled-in resource blob
    gio::resources_register_include!("org.lorebird.app.gresource")
        .expect("Failed to register GResource bundle");

    // 2. Tell GTK's icon theme to also search our resource path
    //    so that icon_name lookups find "org.lorebird.app"
    gtk4::IconTheme::default()
        .add_resource_path("/org/lorebird/app/icons");

    let app = Application::builder()
        .application_id("org.lorebird.app")
        .build();
    // ...
}
```

Step 1 (`gio::resources_register_include!`) expands to a `include_bytes!()` + a
`gio::resources_register()` call that registers the blob compiled by `build.rs`.

Step 2 (`IconTheme::add_resource_path`) tells GTK: "when looking up an icon name,
also look inside this GResource path using the standard icon theme directory layout."
This is what makes the icon show up in the window title bar, task bar, and Alt-Tab
switcher — all via the single `.icon_name()` call on the window.

### 3.1 Setting the window icon

```rust
let window = ApplicationWindow::builder()
    .application(app)
    .title("lorebird")
    .icon_name("org.lorebird.app")   // ← resolves via icon theme → our resource
    .default_width(1200)
    .default_height(700)
    .build();
```

GTK resolves `"org.lorebird.app"` through the icon theme, finds our PNG in the
GResource bundle, and uses it at whatever DPI scaling the compositor requests.

---

## 4. Linux Desktop Integration

For lorebird to appear in GNOME/KDE/etc. application launchers, two things are needed
on the filesystem:

### 4.1 Desktop Entry

`dist/org.lorebird.app.desktop`:

```ini
[Desktop Entry]
Name=lorebird
Comment=Lightweight mail reader for lore.kernel.org
Exec=lorebird
Icon=org.lorebird.app
Type=Application
Categories=Network;Email;
StartupNotify=true
```

### 4.2 Nix package install (`flake.nix`)

The Nix derivation handles `postInstall` to copy icons into the hicolor icon theme,
and `desktopItems` for the `.desktop` file:

```nix
lorebird = pkgs.rustPlatform.buildRustPackage {
  # ...

  nativeBuildInputs = with pkgs; [
    pkg-config
    glib              # provides glib-compile-resources at build time
    wrapGAppsHook4    # wraps the binary with GSettings/GTK env vars
    copyDesktopItems  # installs .desktop files from desktopItems
  ];

  desktopItems = [
    (pkgs.makeDesktopItem {
      name = "org.lorebird.app";
      exec = "lorebird";
      icon = "org.lorebird.app";
      comment = "Lightweight mail reader for lore.kernel.org";
      desktopName = "lorebird";
      categories = [ "Network" "Email" ];
    })
  ];

  postInstall = ''
    for size in 16 32 48 64 128 256; do
      mkdir -p $out/share/icons/hicolor/${size}x${size}/apps
      cp icon/org.lorebird.app.${size}.png \
        $out/share/icons/hicolor/${size}x${size}/apps/org.lorebird.app.png
    done
  '';
};
```

Key points:

- **`glib`** in `nativeBuildInputs` — provides `glib-compile-resources` used by
  `glib_build_tools::compile_resources()` in `build.rs`. Without it, the build fails
  with "glib-compile-resources not found".
- **`wrapGAppsHook4`** — wraps the binary so it can find the GSettings schemas and
  icon themes at runtime. Without it, GTK might not find system themes.
- **`copyDesktopItems`** — installs the `.desktop` file into `$out/share/applications/`.
- **`postInstall`** — copies icons into the hicolor theme so desktop launchers can
  find them. The hicolor theme is the fallback theme that all desktops check.

The GResource bundle (compiled into the binary) provides icons for the running app
itself. The hicolor icons (installed to `$out/share/icons/`) provide icons for the
desktop launcher, since launchers work outside GTK's process and need files on disk.

---

## 5. Windows Integration

The `winres` crate in `build.rs` embeds the `.ico` into the `.exe` PE resource section:

```rust
#[cfg(windows)]
{
    let mut res = winres::WindowsResource::new();
    res.set_icon("resources/org.lorebird.app.ico");
    res.compile().expect("Failed to compile Windows resources");
}
```

This makes the icon appear in:
- Windows Explorer (file manager shows the .exe with the icon)
- Taskbar when the app is running
- Desktop shortcuts
- Alt-Tab switcher

The `.ico` file contains all sizes (16, 32, 48, 64, 128, 256) and Windows selects
the right one for the context (16px for the window title bar, 256px for desktop
shortcuts, etc.).

No runtime code is needed — this is purely a link-time embedding.

---

## 6. macOS Considerations

macOS uses `.icns` files in app bundles, not `.ico`. For a future macOS build:

1. Generate an `.icns` from the PNGs:
   ```bash
   mkdir lorebird.iconset
   cp org.lorebird.app.16.png lorebird.iconset/icon_16x16.png
   cp org.lorebird.app.32.png lorebird.iconset/icon_16x16@2x.png
   cp org.lorebird.app.32.png lorebird.iconset/icon_32x32.png
   cp org.lorebird.app.64.png lorebird.iconset/icon_32x32@2x.png
   # ... etc. for 128, 256, 512 sizes
   iconutil -c icns lorebird.iconset
   ```

2. Set `CFBundleIconFile` in `Info.plist` when creating the `.app` bundle.

This is not currently implemented since the project doesn't target macOS yet.

---

## 7. How to Regenerate Icons

If `icon/lorebird.png` changes:

```bash
cd icon/
for s in 16 32 48 64 128 256; do
    magick lorebird.png -resize ${s}x${s} org.lorebird.app.${s}.png
done
magick lorebird.png -define icon:auto-resize=16,32,48,64,128,256 org.lorebird.app.ico

# Copy into the resource directory
cp org.lorebird.app.*.png ../crates/lorebird-gtk/resources/
cp org.lorebird.app.ico ../crates/lorebird-gtk/resources/
```

Then `cargo clean -p lorebird && cargo build -p lorebird` to recompile the GResource bundle.

---

## 8. Full File Map

```
icon/
├── lorebird.png                          ← source artwork (1024×1024)
├── org.lorebird.app.png                  ← 256×256 copy
├── org.lorebird.app.ico                  ← Windows .ico (multi-size)
└── org.lorebird.app.{16,32,48,64,128,256}.png

crates/lorebird-gtk/
├── build.rs                              ← compile GResource + winres
├── Cargo.toml                            ← glib-build-tools + winres deps
├── resources/
│   ├── org.lorebird.app.gresource.xml    ← manifest
│   ├── org.lorebird.app.{16..256}.png    ← icons in the bundle
│   └── org.lorebird.app.ico              ← Windows icon
└── src/
    ├── main.rs                           ← gio::resources_register_include! + IconTheme path
    └── window.rs                         ← .icon_name("org.lorebird.app")

dist/
└── org.lorebird.app.desktop              ← Linux desktop entry

flake.nix                                 ← nativeBuildInputs, desktopItems, postInstall
```

---

## 9. Debugging

### Verify the GResource bundle was compiled

```bash
# Find the compiled bundle in the build output
find target/debug/build/lorebird-*/out -name '*.gresource'
```

### List resources inside the bundle

```bash
gresource list target/debug/build/lorebird-XXXXXXXX/out/org.lorebird.app.gresource
```

Should output:
```
/org/lorebird/app/icons/scalable/apps/org.lorebird.app.png
/org/lorebird/app/org.lorebird.app.128.png
/org/lorebird/app/org.lorebird.app.16.png
/org/lorebird/app/org.lorebird.app.256.png
/org/lorebird/app/org.lorebird.app.32.png
/org/lorebird/app/org.lorebird.app.48.png
/org/lorebird/app/org.lorebird.app.64.png
```

### Verify the icon shows at runtime

Run the app and check:
```bash
GTK_DEBUG=icontheme lorebird 2>&1 | grep -i 'org.lorebird.app'
```

If GTK can't find the icon, it falls back to a generic "missing image" icon. Check that
`resources_register_include!` is called before the `ApplicationWindow` is built.

### Common pitfalls

| Problem | Cause |
|---------|-------|
| `glib-compile-resources: not found` | Missing `glib` in Nix `nativeBuildInputs` or not on `$PATH` |
| "Failed to register GResource bundle" at startup | `glib-build-tools` version doesn't match `gio` version (must be same major.minor) |
| Icon shows as missing image | `add_resource_path` not called, or the gresource XML path doesn't match the `icons/scalable/apps` layout |
| Windows .exe has no icon | `winres` only runs on Windows targets; cross-compile with `--target x86_64-pc-windows-msvc` |
| Desktop launcher shows generic icon | hicolor icons not installed; check `$out/share/icons/hicolor/` |