"""Run clippy on every Rust target under //crates through Buck.

Builds the `[clippy.txt]` sub-target of each target, prints every non-empty
report, and exits with status 1 when any report is non-empty. Stdlib only.
"""

import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
QUERY = 'kind("rust_(library|binary|test)", //crates/...)'


def buck2(*args):
    return subprocess.run(["buck2", *args], cwd=REPO, check=True, capture_output=True, text=True).stdout


def main():
    targets = [t.strip() for t in buck2("uquery", QUERY).splitlines() if t.strip()]
    if not targets:
        print("no Rust targets found", file=sys.stderr)
        return 1
    subtargets = ["{}[clippy.txt]".format(t) for t in targets]
    output = buck2("build", "--show-full-output", *subtargets)
    failed = False
    for line in output.splitlines():
        target, _, path = line.partition(" ")
        if not path:
            continue
        with open(path, encoding="utf-8", errors="replace") as f:
            report = f.read().strip()
        if report:
            failed = True
            print("{}:\n{}\n".format(target, report))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
