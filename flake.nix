{
  description = "lorebird — index and browse a maildir with GTK, Lua, and SQLite";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      allSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forAllSystems = fn:
        nixpkgs.lib.genAttrs allSystems
          (system: fn {
            pkgs = import nixpkgs { inherit system; };
            inherit system;
          });
    in
    {
      devShells = forAllSystems ({ pkgs, system }: {
        default = pkgs.mkShell {
          name = "lorebird-dev";

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            gtk4
            gtksourceview5
            sqlite-interactive
          ];

          packages = with pkgs; [
            cargo
            rustc
            rust-analyzer
            clippy
            rustfmt
          ];

          shellHook = ''
            echo "=== lorebird dev shell ==="
            echo "Rust:  $(rustc --version)"
            echo "GTK4:  ${pkgs.gtk4.version}"
            echo "GtkSourceView: ${pkgs.gtksourceview5.version}"
            echo "SQLite: $(sqlite3 --version)"
            echo "Lua:   vendored (mlua)"
          '';
        };
      });

      packages = forAllSystems ({ pkgs, system }:
        let
          lorebird = pkgs.rustPlatform.buildRustPackage {
            pname = "lorebird";
            version = "0.1.0";
            src = ./.;

            nativeBuildInputs = with pkgs; [
              pkg-config
              glib
              wrapGAppsHook4
              copyDesktopItems
            ];

            buildInputs = with pkgs; [
              gtk4
              gtksourceview5
            ];

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

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
              # Install icons into the hicolor icon theme
              for size in 16 32 48 64 128 256; do
                mkdir -p $out/share/icons/hicolor/''${size}x''${size}/apps
                cp icon/org.lorebird.app.''${size}.png \
                  $out/share/icons/hicolor/''${size}x''${size}/apps/org.lorebird.app.png
              done
            '';
          };
        in
        {
          default = lorebird;
          lorebird = lorebird;
        });
    };
}
