#!/bin/sh
#MISE description="Run the TUI and watch for changes"
#MISE tools.watchexec="2.3.3"
#
# The TUI bin doesn't have clap so it can't do its own arg parsing
#USAGE flag "--file -f <file>" help="Collection file path"
#USAGE flag "--log-level <level>" default="info" help="Set log level"
#USAGE flag "--tokio-tracing" help="Enable tokio tracing; for use with tokio-console"

FEATURES=""
RUSTFLAGS=""
if [ "${usage_tokio_tracing:-false}" = "true" ]; then
  FEATURES="slumber_util/tokio_tracing"
  RUSTFLAGS="--cfg=tokio_unstable"
fi

set -x
RUSTFLAGS="$RUSTFLAGS" LOG="$usage_log_level" \
    watchexec --on-busy-update=restart --shell=none --wrap-process=none \
    --watch=Cargo.toml --watch=Cargo.lock --watch=crates/ \
    -- \
    cargo run --package slumber_tui --features "$FEATURES" -- $usage_file
