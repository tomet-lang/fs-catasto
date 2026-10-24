{
  flake-parts,
  ...
}@inputs:
flake-parts.lib.mkFlake { inherit inputs; } {
  systems = [
    "x86_64-linux"
    "aarch64-linux"
    "aarch64-darwin"
  ];
  imports = [
    inputs.treefmt-nix.flakeModule
  ];

  perSystem =
    { pkgs, ... }:
    let
      system = pkgs.stdenv.hostPlatform.system;
      craneLib = inputs.crane.mkLib pkgs;
      sisho = inputs.sisho.packages.${system};
      sishoLib = inputs.sisho.legacyPackages.${system};
    in
    {
      packages = rec {
        default = catasto;
        catasto = pkgs.callPackage ./pkgs/catasto.nix { inherit craneLib; };

        # What CI builds and sends to Cachix (.github/workflows/nix-ci.yml).
        ci = pkgs.linkFarm "ci" [
          {
            name = "catasto";
            path = catasto;
          }
        ];
      };

      devShells.default = pkgs.callPackage ./dev.nix {
        inherit inputs;
        fenix = inputs.fenix.packages.${system};
        tools = sishoLib.combine [
          sisho.toolchain
          sisho.tomet-lsp
          sisho.tomet-shelf
        ];
      };

      treefmt = import ./formatter.nix {
        inherit (sisho) tomet;
      };
    };
}
