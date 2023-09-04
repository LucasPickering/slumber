#!/bin/sh

# Run the program with watchexec, for development. Normally we could use
# cargo-watch, but it kills the TUI with SIGKILL so it isn't able to clean up
# after itself, which fucks the terminal. Once cargo-watch is updated to the
# latest watchexec we can get rid of this.
# https://github.com/watchexec/cargo-watch/issues/269

RUST_LOG=slumber=trace watchexec --restart \
    --watch Cargo.toml --watch Cargo.lock --watch src/ --watch slumber.yml \
    -- cargo run
