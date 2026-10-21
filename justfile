# Catasto development tasks.
# Run `just` to see everything available.

default:
    @just --list

#[ Build ]

# Debug build
build:
    cargo build --workspace

# Optimized build
release:
    cargo build --workspace --release

# Build the Nix package
package:
    nix build

#[ Test & lint ]

# Run the test suite
test:
    cargo test --workspace

# Type-check without producing a binary
check:
    cargo check --workspace --all-targets

# Lint. The tree is warning-free; keep it that way.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Format every tracked file (rust, nix, toml, shell, tomet)
fmt:
    nix fmt

#[ Housekeeping ]

# Remove build artifacts
clean:
    cargo clean
    rm -rf dist result

#[ Documentation ]

# Generate and update all derived documentation across the workspace.
docs:
    sisho fix .

# Check that all documents parse, format cleanly, and match export targets.
docs-check:
    sisho check .
