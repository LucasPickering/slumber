#!/usr/bin/env python3
# fmt: off
#MISE description="Generate GIFs from VHS tapes"
#MISE tools=["vhs"]
# fmt: on

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
GIF_MD_FILE = "gifs.md"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
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
    print(f"GIFs will be visible in {GIF_MD_FILE}")

    run(["cargo", "build"])
    # As each GIF is generated, add it to a markdown file so it can be reviewed easily
    with open(GIF_MD_FILE, "w") as f:
        for tape in tapes:
            gif = generate(tape)
            f.write(f"{gif}\n\n![]({gif})\n\n")
            f.flush()

    print(f"Don't forget to check all GIFs in {GIF_MD_FILE} before pushing!")


def generate(tape_path: str) -> str:
    """Generate a single GIF. Return the path to the generated GIF"""
    print("Deleting data/")
    shutil.rmtree("data/")
    run(["vhs", tape_path])
    return get_gif_path(tape_path)


def check_all(tapes: list[str]) -> None:
    """Check all GIFs to see if any are outdated"""
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
    """Get a list of all tape files"""
    return glob.glob(os.path.join(TAPE_DIR, "*"))


def get_tape_path(tape_name: str) -> str:
    """Get path to a tape file by name"""
    return os.path.join(TAPE_DIR, f"{tape_name}.tape")


def get_gif_path(tape_path: str) -> str:
    """Get path to the GIF that a tape generates"""
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
