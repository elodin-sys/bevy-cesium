prepare:
    bash scripts/build_cesium_native.sh

install:
    cargo build --release --workspace --examples

test:
    cargo test --release --workspace

check:
    cargo fmt --all --check
    cargo clippy --release --workspace --all-targets -- -D warnings

viewer:
    cargo run --release --example viewer
