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

#[ Benchmark ]

# Time sync, lock and check on a synthetic workspace of COUNT local files of SIZE_KIB KiB each.
bench count="400" size_kib="512":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release --bin catasto
    bin="$PWD/target/release/catasto"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    mkdir -p "$work/src" "$work/ws"
    for i in $(seq 1 {{count}}); do
        head -c $(({{size_kib}} * 1024)) /dev/urandom > "$work/src/f$i.bin"
        printf '"f%s.bin": { source: "../src/f%s.bin" }\n' "$i" "$i" >> "$work/ws/catasto.ctmt"
    done
    echo "{{count}} files x {{size_kib}} KiB"
    TIMEFORMAT='%R s wall  %U s user'
    for step in sync lock check list; do
        printf '%-6s' "$step"
        time "$bin" "$step" -C "$work/ws" > /dev/null
    done

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
