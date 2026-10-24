# Biblioteca e CLI de debug (`zelador-core`). O helper privilegiado é fixado pelo build:
# a limpeza o chama por caminho do store, nunca pelo PATH.
{
  lib,
  rustPlatform,
  git,
  zelador-helper,
}:
rustPlatform.buildRustPackage {
  pname = "zelador-core";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../clippy.toml
      ../crates
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "--package" "zelador-core" ];
  cargoTestFlags = [ "--package" "zelador-core" ];

  ZELADOR_HELPER = "${zelador-helper}/bin/zelador-helper";

  # Os testes de integração criam repositórios git temporários.
  nativeCheckInputs = [ git ];

  meta.mainProgram = "zelador-core";
}
