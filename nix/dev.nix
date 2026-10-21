{
  pkgs,
  mkShell,
  fenix,
  tools,
  ...
}:
let
  rustToolchain = fenix.combine [
    (fenix.stable.withComponents [
      "cargo"
      "clippy"
      "rustc"
      "rust-src"
      "rustfmt"
      "rust-analyzer"
    ])
  ];
in
mkShell {
  buildInputs = with pkgs; [
    tools
    just

    #= Build
    pkg-config

    #= Rust
    rustToolchain
    cargo-edit
    cargo-outdated
    cargo-nextest
  ];

  shellHook = ''
    echo "🦀📚 Rust Tomet (catasto)"
  '';
}
