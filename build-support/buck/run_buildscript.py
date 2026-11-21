"""Run a first-party build.rs inside its crate tree, as Cargo would.

The working directory and CARGO_MANIFEST_DIR are the crate's directory inside
a tree that holds the repository files at their real relative paths, so build
scripts that read sibling crates through `../` see them. Writes the
`cargo:rustc-cfg`, `rustc-env`, `rustc-link-lib` and `rustc-link-search`
lines in the argument file format the prelude's rustc wrapper reads.
"""

import argparse
import os
import re
import subprocess
import sys


def target_cfgs(rustc, target):
    output = subprocess.run(
        [rustc, "--print", "cfg", "--target", target], check=True, capture_output=True, text=True
    ).stdout
    values = {}
    for line in output.splitlines():
        key, _, value = line.partition("=")
        if not value:
            continue
        values.setdefault(key, []).append(value.strip('"'))
    return values


# Directives with no effect on a Buck build: Buck reruns a script when its
# inputs change, and check-cfg only quiets lints.
IGNORED_DIRECTIVES = {"rerun-if-env-changed", "rustc-check-cfg"}


def relative_to_root(path, out_dir, out_dir_arg):
    """A path inside OUT_DIR as the project-relative path rustc runs with."""
    if path == out_dir or path.startswith(out_dir + os.sep):
        return out_dir_arg + path[len(out_dir) :]
    return path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--buildscript", required=True)
    parser.add_argument("--tree", required=True)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--outfile", required=True)
    parser.add_argument("--rustc", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--feature", action="append", default=[])
    parser.add_argument("--env", action="append", default=[], help="KEY=VALUE for the script")
    args = parser.parse_args()

    manifest_dir = os.path.abspath(os.path.join(args.tree, args.manifest))
    out_dir = os.path.abspath(args.out_dir)
    os.makedirs(out_dir, exist_ok=True)
    rustc = os.path.abspath(args.rustc) if os.sep in args.rustc else args.rustc

    # Only declared inputs reach the script. The ceiling keeps git from
    # finding the checkout around buck-out.
    env = dict(entry.split("=", 1) for entry in args.env)
    env.update(
        {
            "GIT_CEILING_DIRECTORIES": os.path.abspath(args.tree),
            "PATH": os.environ.get("PATH", ""),
            "CARGO": "/bin/false",
            "CARGO_MANIFEST_DIR": manifest_dir,
            "CARGO_PKG_NAME": args.package,
            "CARGO_PKG_VERSION": args.version,
            "DEBUG": "true" if args.profile == "debug" else "false",
            "HOST": args.target,
            "OPT_LEVEL": "0" if args.profile == "debug" else "3",
            "OUT_DIR": out_dir,
            "PROFILE": args.profile,
            "RUSTC": rustc,
            "TARGET": args.target,
        }
    )
    for feature in args.feature:
        env["CARGO_FEATURE_" + feature.upper().replace("-", "_")] = "1"
    for key, values in target_cfgs(rustc, args.target).items():
        env["CARGO_CFG_" + key.upper()] = ",".join(values)

    output = subprocess.run(
        [os.path.abspath(args.buildscript)], cwd=manifest_dir, env=env, check=True, capture_output=True, text=True
    ).stdout

    flags = []
    for line in output.splitlines():
        directive = re.match(r"^cargo::?([a-z-]+)=(.*)$", line)
        if not directive:
            continue
        key, value = directive.groups()
        if key == "rustc-cfg":
            flags.append("--cfg=" + value)
        elif key == "rustc-env":
            name, _, var = value.partition("=")
            flags.append("--env-set={}={}".format(name, var))
        elif key == "rustc-link-lib":
            flags.append("-l" + value)
        elif key == "rustc-link-search":
            kind, _, path = value.rpartition("=")
            flags.append("-L{}{}".format(kind + "=" if kind else "", relative_to_root(path, out_dir, args.out_dir)))
        elif key == "rerun-if-changed":
            # A path the script read that the tree lacks means the crate's
            # tree in its BUCK file is stale.
            if not os.path.exists(os.path.join(manifest_dir, value)):
                sys.exit(
                    "{}: build script read {}, which is not in its crate tree; "
                    "run scripts/update-buck.py --first-party".format(args.package, value)
                )
        elif key == "warning":
            print("{}: warning: {}".format(args.package, value), file=sys.stderr)
        elif key not in IGNORED_DIRECTIVES:
            sys.exit("{}: unsupported build script directive cargo:{}".format(args.package, key))
    with open(args.outfile, "w", encoding="utf-8", newline="\n") as f:
        f.write("".join(flag + "\n" for flag in flags))
    return 0


if __name__ == "__main__":
    sys.exit(main())
