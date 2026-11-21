{
  config,
  lib,
  pkgs,
  ...
}:

let
  llvm = pkgs.llvmPackages;

  steamRuntimeSdk = import ./build-support/nix/steamrt-sysroot.nix {
    inherit pkgs;
    name = "steamrt4-sdk-sysroot";
    archiveUrl = "https://repo.steampowered.com/steamrt4/images/4.0.20260805.254769/com.valvesoftware.SteamRuntime.Sdk-amd64,i386-steamrt4-sysroot.tar.gz";
    archiveHash = "sha256-0QXlecEmF1VxS0A7r9hC6ayz151aE9+GlpOBikm3I3k=";
    sdk = true;
  };

  steamRuntimePlatform = import ./build-support/nix/steamrt-sysroot.nix {
    inherit pkgs;
    name = "steamrt4-platform-sysroot";
    archiveUrl = "https://repo.steampowered.com/steamrt4/images/4.0.20260805.254769/com.valvesoftware.SteamRuntime.Platform-amd64,i386-steamrt4-sysroot.tar.gz";
    archiveHash = "sha256-tKNTn9MTFmWF8IL9y8pFaFfWp9AJEMkJAVVegtqDWsY=";
  };

  # Unwrapped clang: the Nix wrapper adds Nix glibc paths, which a SteamOS
  # binary must not reference. The link flags also reach `-c` compiles, so
  # the unused-argument warning is off for build scripts that use -Werror.
  steamRuntimeCc = pkgs.writeShellScriptBin "steamrt-cc" ''
    exec ${llvm.clang-unwrapped}/bin/clang \
      --sysroot=${steamRuntimeSdk} \
      --target=x86_64-unknown-linux-gnu \
      -resource-dir=${llvm.clang}/resource-root \
      --ld-path=${llvm.lld}/bin/ld.lld \
      -Wl,--dynamic-linker=/lib64/ld-linux-x86-64.so.2 \
      -Wno-unused-command-line-argument \
      "$@"
  '';

  steamRuntimeCxx = pkgs.writeShellScriptBin "steamrt-cxx" ''
    sysroot=${steamRuntimeSdk}
    version=$(find "$sysroot/usr/include/c++" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort -V | tail -n 1)
    if [ -z "$version" ]; then
      echo "Steam Runtime SDK C++ headers were not found" >&2
      exit 1
    fi
    exec ${llvm.clang-unwrapped}/bin/clang++ \
      --sysroot="$sysroot" \
      --target=x86_64-unknown-linux-gnu \
      -resource-dir=${llvm.clang}/resource-root \
      -nostdinc++ \
      -isystem "$sysroot/usr/include/c++/$version" \
      -isystem "$sysroot/usr/include/c++/$version/backward" \
      -isystem "$sysroot/usr/include/x86_64-linux-gnu/c++/$version" \
      --ld-path=${llvm.lld}/bin/ld.lld \
      -Wl,--dynamic-linker=/lib64/ld-linux-x86-64.so.2 \
      -Wno-unused-command-line-argument \
      "$@"
  '';

  # The game module toolchain `h2 build` runs: clang, ld.lld and llvm-ar on
  # PATH, compiling against the Steam Runtime sysroot (H2_LINUX_SYSROOT).
  moduleToolchain = pkgs.runCommand "h2-module-toolchain" { } ''
    mkdir -p "$out/bin"
    cat > "$out/bin/clang" <<EOF
    #!${pkgs.runtimeShell}
    exec ${llvm.clang-unwrapped}/bin/clang -resource-dir=${llvm.clang}/resource-root --ld-path=${llvm.lld}/bin/ld.lld -Wno-unused-command-line-argument "\$@"
    EOF
    chmod +x "$out/bin/clang"
    ln -s ${llvm.lld}/bin/ld.lld "$out/bin/ld.lld"
    ln -s ${llvm.bintools-unwrapped}/bin/llvm-ar "$out/bin/llvm-ar"
  '';

  nativeLibraries = with pkgs; [
    alsa-lib
    libxkbcommon
    udev
    vulkan-loader
    wayland
    libx11
    libxcursor
    libxi
    libxrandr
  ];

  nativeLibraryLinkerFlags = lib.concatMapStringsSep "," (
    package: "-L${lib.getLib package}/lib"
  ) nativeLibraries;

  rust = config.languages.rust.toolchainPackage;

  # Included by .buckconfig through the .buckconfig.devenv link that
  # enterShell maintains.
  buckConfig = pkgs.writeText "h2r-buckconfig" ''
    [host_toolchain]
      ar = ${llvm.bintools-unwrapped}/bin/llvm-ar
      cc = ${llvm.clang}/bin/clang
      clippy_driver = ${rust}/bin/clippy-driver
      compiler_type = clang
      cxx = ${llvm.clang}/bin/clang++
      linker_flags = ${nativeLibraryLinkerFlags}
      native_link = ${llvm.clang}/bin/clang++
      python = ${pkgs.python3}/bin/python3
      pkg_config = ${pkgs.pkg-config}/bin/pkg-config
      rustc = ${rust}/bin/rustc
      rustc_target_triple = ${pkgs.stdenv.hostPlatform.config}
      rustdoc = ${rust}/bin/rustdoc
      sevenzip = ${pkgs.p7zip}/bin/7z
      module_toolchain = ${moduleToolchain}/bin
      readelf = ${llvm.bintools-unwrapped}/bin/llvm-readelf
      steamos_ar = ${llvm.bintools-unwrapped}/bin/llvm-ar
      steamos_cc = ${steamRuntimeCc}/bin/steamrt-cc
      steamos_compiler_type = clang
      steamos_cxx = ${steamRuntimeCxx}/bin/steamrt-cxx
      steamos_native_link = ${steamRuntimeCxx}/bin/steamrt-cxx
      steamos_pkg_config_libdir = ${steamRuntimeSdk}/usr/lib/x86_64-linux-gnu/pkgconfig:${steamRuntimeSdk}/usr/lib/pkgconfig:${steamRuntimeSdk}/usr/share/pkgconfig
      steamos_sysroot = ${steamRuntimeSdk}
      steamos_platform_sysroot = ${steamRuntimePlatform}
  '';

in
{
  assertions = [
    {
      assertion = pkgs.stdenv.hostPlatform.system == "x86_64-linux";
      message = "The devenv shell supports x86_64 Linux.";
    }
  ];

  languages.rust = {
    enable = true;
    toolchainFile = ./rust-toolchain.toml;
  };

  packages =
    with pkgs;
    [
      llvm.clang
      llvm.lld
      llvm.bintools-unwrapped
      avahi
      openssh
      p7zip
      pkg-config
      python3
      reindeer
      rsync
    ]
    ++ nativeLibraries;

  env = {
    LD_LIBRARY_PATH = lib.makeLibraryPath nativeLibraries;
    PKG_CONFIG_PATH = lib.makeSearchPathOutput "dev" "lib/pkgconfig" nativeLibraries;
  };

  enterShell = ''
    ln -sfn ${buckConfig} "$DEVENV_ROOT/.buckconfig.devenv"
  '';

  enterTest = ''
    rustc --version | grep -F "rustc 1.98.1"
    buck2 root >/dev/null
    pkg-config --exists alsa libudev
    7z i >/dev/null
    command -v ssh ssh-keygen rsync avahi-browse avahi-resolve >/dev/null
    test -x ${steamRuntimeSdk}/usr/bin/pkg-config
    test -f ${steamRuntimeSdk}/usr/include/alsa/asoundlib.h
    test -e ${steamRuntimePlatform}/lib64/ld-linux-x86-64.so.2
    ${moduleToolchain}/bin/clang --version >/dev/null
  '';
}
