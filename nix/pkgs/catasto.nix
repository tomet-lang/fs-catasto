{
  craneLib,
  lib,
  makeWrapper,
  gitMinimal,
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

    # The tests build their own git repositories and sync from them.
    nativeCheckInputs = [ gitMinimal ];

    # catasto reads git sources through `git`. A git on the user's PATH comes
    # first, with its credential helpers; this one is what the package falls
    # back to, so `catasto sync` works where git is not installed.
    nativeBuildInputs = [ makeWrapper ];
    postInstall = ''
      wrapProgram $out/bin/catasto --suffix PATH : ${lib.makeBinPath [ gitMinimal ]}
    '';
  }
)
