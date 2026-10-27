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
      tometPkgs = inputs.tomet.packages.${system};
      twritPkgs = inputs.twrit.packages.${system};
      shelfPkgs = inputs.tomet-shelf.packages.${system};
      sishoPkgs = inputs.sisho.packages.${system};
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
        tools = pkgs.symlinkJoin {
          name = "tomet-toolchain";
          paths = [
            sishoPkgs.sisho
            tometPkgs.tomet
            tometPkgs.tomet-lsp
            twritPkgs.twrit
            shelfPkgs.tomet-shelf
          ];
        };
      };

      treefmt = import ./formatter.nix {
        inherit (tometPkgs) tomet;
      };
    };
}
