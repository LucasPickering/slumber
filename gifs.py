#!/usr/bin/env python3

"""
Generate GIFs from VHS tapes
"""

import argparse
import glob
import os
import re
import shutil
import subprocess

TAPE_DIR = "tapes/"
OUTPUT_REGEX = re.compile(r"^Output \"(?P<path>.*)\"$")


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate GIFs for docs")
    parser.add_argument(
        "--check", action="store_true", help="Check if all GIFs are up to date"
    )
    parser.add_argument("tapes", nargs="*", help="Generate or check specific tapes")
    args = parser.parse_args()

    tapes = [get_tape_path(tape) for tape in args.tapes]
    if args.check:
        check_all(tapes)
    else:
        generate_all(tapes)


def generate_all(tapes: list[str]) -> None:
    if not tapes:
        tapes = get_tapes()
    print(f"Generating GIFs for: {tapes}")

    run(["cargo", "build"])
    for tape in tapes:
        generate(tape)
    print("Don't forget to check all GIFs before pushing!")


def generate(tape: str) -> None:
    print("Deleting data/")
    shutil.rmtree("data/")
    run(["vhs", tape])


def check_all(tapes: list[str]) -> None:
    if not tapes:
        tapes = get_tapes()
    latest_commit = run(["git", "rev-parse", "HEAD"])
    failed = []
    for tape in tapes:
        gif = get_gif_path(tape)
        good = check(gif_path=gif, latest_commit=latest_commit)
        if not good:
            failed.append(gif)
        print(f"  {tape} -> {gif}: {'PASS' if good else 'FAIL'}")
    if failed:
        raise Exception(f"Some GIFs are outdated: {failed}")
    else:
        print("All GIFs are up to date")


def check(gif_path: str, latest_commit: str) -> bool:
    """Check if the GIF is outdated"""
    latest_gif_commit = run(
        ["git", "log", "-n", "1", "--pretty=format:%H", "--", gif_path]
    )
    return latest_commit == latest_gif_commit


def get_tapes() -> list[str]:
    return glob.glob(os.path.join(TAPE_DIR, "*"))


def get_tape_path(tape_name: str) -> str:
    return os.path.join(TAPE_DIR, f"{tape_name}.tape")


def get_gif_path(tape_path: str) -> str:
    with open(tape_path) as f:
        for line in f:
            m = OUTPUT_REGEX.match(line)
            if m:
                return m.group("path")
    raise ValueError(f"Tape file {tape_path} missing Output declaration")


def run(command: list[str]) -> str:
    output = subprocess.check_output(command)
    return output.decode().strip()


if __name__ == "__main__":
    main()
