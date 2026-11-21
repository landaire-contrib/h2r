{
  pkgs,
  name,
  archiveUrl,
  archiveHash,
  sdk ? false,
}:

let
  sysrootArchive = pkgs.fetchurl {
    name = "${name}.tar.gz";
    url = archiveUrl;
    hash = archiveHash;
  };

  sdkChecks = if sdk then ''
    test -x "$out/usr/bin/pkg-config"
    test -f "$out/usr/include/alsa/asoundlib.h"
  '' else "";
in
pkgs.runCommand name
  { }
  ''
    mkdir -p "$out"
    tar -xzf "${sysrootArchive}" -C "$out" --no-same-owner --no-same-permissions
    find "$out" -type d -exec chmod u+rwx {} +
    while IFS= read -r -d $'\\0' link; do
      target=$(readlink "$link")
      case "$target" in
        /*)
          resolved=$(realpath -ms "$out$target")
          case "$resolved" in
            "$out"/*) ;;
            *) echo "Steam Runtime symlink escapes the sysroot: $link -> $target" >&2; exit 1 ;;
          esac
          relative=$(realpath -ms --relative-to="$(dirname "$link")" "$resolved")
          ln -sfn "$relative" "$link"
          ;;
      esac
    done < <(find "$out" -type l -lname '/*' -print0)
    test -e "$out/lib64/ld-linux-x86-64.so.2"
    ${sdkChecks}
  ''
