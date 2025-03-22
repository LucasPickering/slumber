#!/bin/sh

# Run the TUI with watchexec, for development. Normally we could use
# cargo-watch, but it kills the TUI with SIGKILL so it isn't able to clean up
# after itself, which fucks the terminal. Once cargo-watch is updated to the
# latest watchexec we can get rid of this.
# https://github.com/watchexec/cargo-watch/issues/269

RUST_LOG=${RUST_LOG:-slumber=${LOG:-DEBUG}} RUST_BACKTRACE=1 watchexec --restart --no-process-group \
    --watch Cargo.toml --watch Cargo.lock --watch src/ --watch crates/ \
    -- cargo run --no-default-features --features tui \
    -- $@
