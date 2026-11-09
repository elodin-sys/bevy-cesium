prepare:
    bash scripts/build_cesium_native.sh

install target="native":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{target}}" in
      native) cargo build --release --workspace --examples ;;
      web) env -u NO_COLOR TRUNK_COLOR=never trunk build --config web/Trunk.toml --example browser ;;
      *) echo "usage: just install [native|web]" >&2; exit 1 ;;
    esac

web:
    env -u NO_COLOR TRUNK_COLOR=never trunk serve --config web/Trunk.toml --example browser

test:
    cargo test --release --workspace

check:
    cargo fmt --all --check
    cargo clippy --release --workspace --all-targets -- -D warnings

viewer:
    cargo run --release --example viewer
