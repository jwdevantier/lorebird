{
  description = "loreread — index and browse a maildir with GTK, Lua, and SQLite";

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
          name = "loreread-dev";

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
            echo "=== loreread dev shell ==="
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
          loreread = pkgs.rustPlatform.buildRustPackage {
            pname = "loreread";
            version = "0.1.0";
            src = ./.;

            nativeBuildInputs = with pkgs; [
              pkg-config
            ];

            buildInputs = with pkgs; [
              gtk4
              gtksourceview5
            ];

            cargoLock = {
              lockFile = ./Cargo.lock;
            };
          };
        in
        {
          default = loreread;
          loreread = loreread;
        });
    };
}
