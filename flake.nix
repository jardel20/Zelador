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
      packages = forAllSystems (pkgs: {
        # Fase 1: só o crate `zelador-core` (biblioteca e CLI de debug).
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "zelador-core";
          version = "0.1.0";
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./clippy.toml
              ./crates
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--package" "zelador-core" ];
          cargoTestFlags = [ "--package" "zelador-core" ];
          # Os testes de integração criam repositórios git temporários.
          nativeCheckInputs = [ pkgs.git ];
          meta.mainProgram = "zelador-core";
        };
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
          ];
        };
      });
    };
}
