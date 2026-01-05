#!/bin/sh
#MISE description="Run the TUI and watch for changes"
#MISE tools=["watchexec"]

FEATURES="tui"
RUSTFLAGS=""
if [ "$TRACING" = "true" ]; then
  FEATURES="$FEATURES,tokio_tracing"
  RUSTFLAGS="--cfg=tokio_unstable"
fi

RUSTFLAGS="$RUSTFLAGS" \
    exec watchexec --restart --no-process-group \
    --watch Cargo.toml --watch Cargo.lock --watch src/ --watch crates/ \
    -- \
    cargo run --no-default-features --features "$FEATURES" -- $@
