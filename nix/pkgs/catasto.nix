{
  craneLib,
}:
let
  src = ../..;

  commonArgs = {
    inherit src;

    pname = "catasto";
    version = "0.1.0";
  };

  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;

    cargoExtraArgs = "-p catasto-cli";

    doCheck = true;
  }
)
