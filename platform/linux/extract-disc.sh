#!/usr/bin/env bash
# Writes the disc named by [h2] disc (an ISO, an XISO, or a .7z holding one)
# to $OUT. Usage: extract-disc.sh <source> <7z>
set -euo pipefail
source="${1/#\~/$HOME}"
sevenzip=$2
if [ -z "$source" ]; then
  echo "Set [h2] disc in .buckconfig.local to your Halo 2 disc image (.iso or .7z)." >&2
  exit 1
fi
shopt -s nocasematch
case "$source" in
  *.7z)
    entry=$("$sevenzip" l -slt "$source" | sed -n 's/^Path = \(.*\.[iI][sS][oO]\)$/\1/p' | head -n 1)
    if [ -z "$entry" ]; then
      echo "$source holds no .iso" >&2
      exit 1
    fi
    "$sevenzip" e -so "$source" "$entry" > "$OUT"
    ;;
  *)
    cp --reflink=auto "$source" "$OUT"
    ;;
esac
