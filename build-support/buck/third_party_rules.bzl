load("@prelude//rust:cargo_buildscript.bzl", _buildscript_run = "buildscript_run")
load("@prelude//rust:cargo_package.bzl", _cargo = "cargo")
load("@toolchains//:host_toolchain.bzl", "buildscript_env")

def buildscript_run(env = {}, **kwargs):
    # The configured toolchain overrides fixup-declared compilers and profile.
    _buildscript_run(env = env | buildscript_env(), **kwargs)

def _rust_library(rustc_flags = [], **kwargs):
    # Cargo.toml builds every dependency at opt-level 3, including in dev.
    _cargo.rust_library(rustc_flags = ["-Copt-level=3"] + rustc_flags, **kwargs)

cargo = struct(
    rust_binary = _cargo.rust_binary,
    rust_library = _rust_library,
)
