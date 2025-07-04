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
    subparsers = parser.add_subparsers(help="Subcommand to execute")

    # `cli` subcommand
    cli_parser = subparsers.add_parser("cli", help=cli.__doc__)
    cli_parser.add_argument(
        "args", nargs=argparse.REMAINDER, help="Additional arguments to pass to the CLI"
    )
    cli_parser.set_defaults(func=cli)

    # `tui` subcommand
    tui_parser = subparsers.add_parser("tui", help=tui.__doc__)
    tui_parser.set_defaults(func=tui)
    tui_parser.add_argument(
        "--tracing",
        action="store_true",
        help="Enable tokio tracing. Requires setting the tokio_unstable compiler flag,"
        " so this will trigger a full recompilation",
    )
    tui_parser.add_argument(
        "args", nargs=argparse.REMAINDER, help="Additional arguments to pass to the TUI"
    )

    args = vars(parser.parse_args())

    # Defer to subcommand function
    func = args.pop("func")
    func(**args)


def cli(log: str, args: list[str]) -> None:
    """Run a CLI command"""
    cargo_command = [
        "cargo",
        "run",
        "--no-default-features",
        "--features",
        "cli",
        "--",
        # Forward args from the user
        *args,
    ]

    result = subprocess.run(cargo_command, env=env(log))
    sys.exit(result.returncode)


def tui(log: str, tracing: bool, args: list[str]) -> None:
    """Run the TUI with watchexec"""
    cargo_command = [
        "cargo",
        "run",
        "--no-default-features",
        "--features",
        "tui",
        *(["--features", "tokio_tracing"] if tracing else []),
        "--",
        # Forward args from the user
        *args,
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
                "RUSTFLAGS": "--cfg=tokio_unstable" if tracing else "",
                **env(log),
            },
        )
        sys.exit(result.returncode)
    except KeyboardInterrupt:
        sys.exit(0)


def env(log: str) -> dict[str, str]:
    """Get the environment that cargo should be run with"""
    return {
        "RUST_LOG": f"slumber={log}",
        "RUST_BACKTRACE": "1",
        **os.environ,
    }


if __name__ == "__main__":
    main()
