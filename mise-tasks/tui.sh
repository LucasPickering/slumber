#!/bin/sh
#MISE description="Run the TUI and watch for changes"
#MISE tools.watchexec="2.3.3"

FEATURES="tui"
RUSTFLAGS=""
if [ "$TRACING" = "true" ]; then
  FEATURES="$FEATURES,tokio_tracing"
  RUSTFLAGS="--cfg=tokio_unstable"
fi

RUSTFLAGS="$RUSTFLAGS" \
    watchexec --on-busy-update=restart --shell=none --wrap-process=none \
    --watch=Cargo.toml --watch=Cargo.lock --watch=src/ --watch=crates/ \
    -- \
    cargo run --no-default-features --features "$FEATURES" -- $@
