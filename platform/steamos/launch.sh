#!/bin/sh
# Starts the game on SteamOS from the directory //platform/steamos:run fills.
# The module is prebuilt: the device has no compiler, so H2_OPT must match
# the value the module was built with (h2-opt, beside this script).
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
opt=$(cat "$root/h2-opt")
if [ -n "$opt" ]; then
  export H2_OPT="$opt"
fi
# KEY=VALUE lines from `run --env`: Steam does not pass a shortcut's
# environment into the runtime container.
if [ -f "$root/h2-env" ]; then
  while IFS= read -r line; do
    if [ -n "$line" ]; then
      export "$line"
    fi
  done < "$root/h2-env"
fi
exec "$root/h2" play "$root/disc/halo2.iso" --out "$root/module" --tier clang "$@"
