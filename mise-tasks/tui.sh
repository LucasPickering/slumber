#!/bin/sh
#MISE description="Run the TUI and watch for changes"
#
# The TUI bin doesn't have clap so it can't do its own arg parsing
#USAGE flag "--file -f <file>" help="Collection file path"
#USAGE flag "--log-level <level>" default="info" help="Set log level"
#USAGE flag "--tokio-tracing" help="Enable tokio tracing; for use with tokio-console"
#USAGE flag "--profile" help="Run the TUI with samply for profiling"

export LOG="$usage_log_level"
FEATURES=""
export RUSTFLAGS=""
if [ "${usage_tokio_tracing:-false}" = "true" ]; then
  FEATURES="slumber_util/tokio_tracing"
  RUSTFLAGS="--cfg=tokio_unstable"
fi

if [ "${usage_profile:-false}" = "true" ]; then
  # Profiling - build a release binary and run it under samply
  export CARGO_PROFILE_RELEASE_DEBUG=true # Include debug symbols
  export SLUMBER_DATA_DIRECTORY="${MISE_PROJECT_ROOT}/data/" # Use local config/data dir
  cargo build --package slumber_tui --features "$FEATURES" --release
  samply record ./target/release/slumber_tui $usage_file
else
  # Not profiling - run regularly & watch for changes
    watchexec --on-busy-update=restart --shell=none --wrap-process=none \
    --watch=Cargo.toml --watch=Cargo.lock --watch=crates/ \
    -- \
    cargo run --package slumber_tui --features "$FEATURES" -- $usage_file
fi
