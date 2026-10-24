{
  description = "Zelador — painel operacional para NixOS";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      nixosModules.default = import ./nix/module.nix { inherit self; };

      checks.x86_64-linux.helper-vm = import ./nix/vm-test.nix {
        pkgs = nixpkgs.legacyPackages.x86_64-linux;
        module = self.nixosModules.default;
      };

      packages = forAllSystems (pkgs: {

        zelador-helper = pkgs.callPackage ./nix/helper.nix { };
        zelador-core = pkgs.callPackage ./nix/core.nix {
          zelador-helper = self.packages.${pkgs.stdenv.hostPlatform.system}.zelador-helper;
        };
        zelador = pkgs.callPackage ./nix/app.nix {
          zelador-helper = self.packages.${pkgs.stdenv.hostPlatform.system}.zelador-helper;
        };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.zelador-core;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            pkg-config
            gtk4
            libadwaita
            git
            nvd
            gettext
          ];
          # Fontes da identidade visual visíveis ao fontconfig no devShell. No sistema, o
          # módulo NixOS as instala em fonts.packages.
          FONTCONFIG_FILE = pkgs.makeFontsConf {
            fontDirectories = [
              pkgs.poppins
              pkgs.jetbrains-mono
            ];
          };
        };
      });
    };
}
