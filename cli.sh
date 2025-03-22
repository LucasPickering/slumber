#!/bin/sh

# Run the CLI, for development

RUST_LOG=${RUST_LOG:-slumber=${LOG:-DEBUG}} RUST_BACKTRACE=1 \
    cargo run --no-default-features --features cli \
    -- $@
