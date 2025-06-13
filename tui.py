#!/usr/bin/env python3

"""
Run the TUI with watchexec for development. This exists to make it easier to set some common
environment variables and flags for development. This will not enable the `cli` feature, so it
will compile a bit faster than the full binary.
"""

import os
import sys
import subprocess
import argparse
import itertools

WATCH_PATHS = ["Cargo.toml", "Cargo.lock", "src/", "crates/"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", "-l", default="DEBUG", help="Log level")
    parser.add_argument(
        "--tracing",
        action="store_true",
        help="Enable tokio tracing. Requires setting the tokio_unstable compiler flag,"
        " so this will trigger a full recompilation",
    )
    parser.add_argument(
        "args", nargs=argparse.REMAINDER, help="Additional arguments to pass to the TUI"
    )
    args = parser.parse_args()

    cargo_command = [
        "cargo",
        "run",
        "--no-default-features",
        "--features",
        "tui",
        *(["--features", "tokio_tracing"] if args.tracing else []),
        "--",
        # Forward args from the user
        *args.args,
    ]

    watchexec_command: list[str] = [
        "watchexec",
        "--restart",
        "--no-process-group",
        *(itertools.chain.from_iterable(["--watch", path] for path in WATCH_PATHS)),
        "--",
        *cargo_command,
    ]

    try:
        result = subprocess.run(
            watchexec_command,
            env={
                "RUST_LOG": f"slumber={args.log}",
                "RUST_BACKTRACE": "1",
                "RUSTFLAGS": "--cfg=tokio_unstable" if args.tracing else "",
                **os.environ,
            },
        )
        sys.exit(result.returncode)
    except KeyboardInterrupt:
        sys.exit(0)


if __name__ == "__main__":
    main()
