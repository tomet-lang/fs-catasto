{ tomet, ... }:
{
  projectRootFile = "flake.nix";
  programs = {
    #--- Nix ---
    nixfmt.enable = true;
    statix.enable = true;
    deadnix.enable = true;
    #--- Shell ---
    shfmt.enable = true;
    shellcheck.enable = true;

    #--- Main ---
    rustfmt.enable = true;
    taplo = {
      enable = true;
      settings = {
        formatting = {
          align_entries = true;
          align_comments = true;
          reorder_keys = false;
          column_width = 120;
        };
        rule = [
          {
            include = [ "Cargo.toml" ];
            keys = [
              "dependencies"
              "dev-dependencies"
              "build-dependencies"
            ];
            formatting = {
              reorder_keys = true;
            };
          }
        ];
      };
    };
  };

  settings = {
    global.excludes = [
      "*.lock"
    ];

    formatter = {
      tomet = {
        command = "${tomet}/bin/tomet";
        options = [
          "format"
          "-i"
        ];
        includes = [ "*.tmt" ];
      };
    };

    shfmt = {
      includes = [ "*.sh" ];
    };

    rustfmt = {
      includes = [ "*.rs" ];
    };
  };
}
