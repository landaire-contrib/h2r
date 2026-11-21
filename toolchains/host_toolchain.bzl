load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerType")
load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")
load("@prelude//toolchains:cxx.bzl", "CxxToolsInfo")

# Every tool is an absolute path in [host_toolchain] of .buckconfig.local, which
# scripts/configure-buck-toolchain.py or the devenv shell writes. Nothing is
# resolved through PATH.
_SECTION = "host_toolchain"
_STEAMOS_TOOLS = ["ar", "cc", "compiler_type", "cxx", "native_link"]

_BOOTSTRAP_HINT = "Run `python3 scripts/configure-buck-toolchain.py` or enter `devenv shell` before invoking Buck2."

def steamos_enabled():
    """Build against the Steam Runtime sysroot, so binaries run on SteamOS and
    on any newer glibc. On by default where the toolchain has the sysroot (the
    devenv shell): Nix's own glibc cannot load the host's GPU drivers."""
    default = "true" if read_root_config(_SECTION, "steamos_sysroot") != None else "false"
    return read_root_config("build_mode", "steamos", default) == "true"

def host_tool(name, optional = False):
    key = "steamos_" + name if steamos_enabled() and name in _STEAMOS_TOOLS else name
    path = read_root_config(_SECTION, key)
    if path == None:
        if optional:
            return None
        fail("Missing [{}] {}. {}".format(_SECTION, key, _BOOTSTRAP_HINT))
    return path

def host_linker_flags():
    if steamos_enabled():
        return []
    flags = host_tool("linker_flags", optional = True)
    if flags == None or flags == "":
        return []
    return flags.split(",")

def steamos_linker_flags():
    if not steamos_enabled():
        return []
    sysroot = host_tool("steamos_sysroot")
    return [
        "--sysroot=" + sysroot,
        "-fuse-ld=lld",
        "-Wl,--dynamic-linker=/lib64/ld-linux-x86-64.so.2",
    ]

def _is_windows():
    return host_info().os.is_windows

def build_mode():
    mode = read_root_config("build_mode", "mode", "debug")
    if mode not in ["debug", "release"]:
        fail("[build_mode] mode must be debug or release, got {}".format(mode))
    return mode

def _rustc_flags():
    if build_mode() == "release":
        return ["-Copt-level=3", "-Cdebuginfo=0"]

    # Cargo's dev profile as the root Cargo.toml sets it.
    return ["-Copt-level=0", "-Cdebuginfo=line-tables-only", "-Cdebug-assertions=on", "-Coverflow-checks=on"]

def buildscript_env():
    """Environment for every third-party build script.

    The prelude's CC/CXX shims add clang/gcc-only flags that cl.exe rejects, so
    the compilers are passed directly, together with the profile values cc-rs
    reads.
    """
    # Third-party crates build at opt-level 3 in both modes, as Cargo's
    # [profile.dev.package."*"] specifies.
    if build_mode() == "release":
        env = {"DEBUG": "false", "OPT_LEVEL": "3", "PROFILE": "release"}
    else:
        env = {"DEBUG": "true", "OPT_LEVEL": "3", "PROFILE": "debug"}
    for var, key in [("AR", "ar"), ("CC", "cc"), ("CXX", "cxx")]:
        env[var] = host_tool(key)
    for var, key in [("INCLUDE", "include"), ("LIB", "lib_paths")]:
        value = host_tool(key, optional = True)
        if value != None:
            env[var] = value
    pkg_config = host_tool("pkg_config", optional = True)
    if pkg_config != None:
        env["PKG_CONFIG"] = pkg_config
    if steamos_enabled():
        env["NIX_LDFLAGS"] = ""
        env["PKG_CONFIG_PATH"] = ""
        env["PKG_CONFIG_LIBDIR"] = host_tool("steamos_pkg_config_libdir")
        env["PKG_CONFIG_SYSROOT_DIR"] = host_tool("steamos_sysroot")
        env["PKG_CONFIG_ALLOW_CROSS"] = "1"
    return env

ToolPathsInfo = provider(fields = {
    "clippy_driver": provider_field(str),
    "rustc": provider_field(str),
    "rustdoc": provider_field(str),
})

def _msvc_wrapper(ctx, name, executable):
    # cl.exe and link.exe read their search paths from the environment. The
    # wrapper sets exactly those and clears PATH.
    content = cmd_args(
        "@echo off\r\nsetlocal DisableDelayedExpansion\r\nset \"PATH=\"\r\nset \"INCLUDE=",
        ctx.attrs.include,
        "\"\r\nset \"LIB=",
        ctx.attrs.lib_paths,
        "\"\r\nset \"LIBPATH=%LIB%\"\r\n\"",
        executable,
        "\" %*\r\n",
        delimiter = "",
    )
    wrapper, _ = ctx.actions.write(name + ".bat", content, allow_args = True)
    return wrapper

def _host_tools_impl(ctx):
    paths = ToolPathsInfo(
        clippy_driver = ctx.attrs.clippy_driver,
        rustc = ctx.attrs.rustc,
        rustdoc = ctx.attrs.rustdoc,
    )
    if ctx.attrs.windows:
        cl = _msvc_wrapper(ctx, "cl", ctx.attrs.cc)
        tools = CxxToolsInfo(
            compiler = cl,
            compiler_type = "windows",
            cxx_compiler = cl,
            asm_compiler = _msvc_wrapper(ctx, "ml64", ctx.attrs.ml64),
            asm_compiler_type = "windows_ml64",
            rc_compiler = _msvc_wrapper(ctx, "rc", ctx.attrs.rc),
            cvtres_compiler = _msvc_wrapper(ctx, "cvtres", ctx.attrs.cvtres),
            archiver = _msvc_wrapper(ctx, "lib", ctx.attrs.ar),
            archiver_type = "windows",
            linker = _msvc_wrapper(ctx, "link", ctx.attrs.link),
            linker_type = LinkerType("windows"),
        )
    else:
        tools = CxxToolsInfo(
            compiler = ctx.attrs.cc,
            compiler_type = ctx.attrs.compiler_type,
            cxx_compiler = ctx.attrs.cxx,
            asm_compiler = ctx.attrs.cc,
            asm_compiler_type = ctx.attrs.compiler_type,
            rc_compiler = None,
            cvtres_compiler = None,
            archiver = ctx.attrs.ar,
            archiver_type = "gnu",
            linker = ctx.attrs.link,
            linker_type = LinkerType(ctx.attrs.linker_type),
        )
    return [DefaultInfo(), tools, paths]

_host_tools_rule = rule(
    impl = _host_tools_impl,
    attrs = {
        "ar": attrs.string(),
        "cc": attrs.string(),
        "clippy_driver": attrs.string(),
        "compiler_type": attrs.string(),
        "cvtres": attrs.string(),
        "cxx": attrs.string(),
        "include": attrs.string(),
        "lib_paths": attrs.string(),
        "link": attrs.string(),
        "linker_type": attrs.string(),
        "ml64": attrs.string(),
        "rc": attrs.string(),
        "rustc": attrs.string(),
        "rustdoc": attrs.string(),
        "windows": attrs.bool(),
    },
)

def host_tools(name, visibility):
    # read_root_config is only available while the package loads, so every
    # path is resolved here and passed in as an attribute.
    windows = _is_windows()
    if windows:
        native_attrs = {
            "compiler_type": "windows",
            "cvtres": host_tool("cvtres"),
            "cxx": host_tool("cxx"),
            "include": host_tool("include"),
            "lib_paths": host_tool("lib_paths"),
            "link": host_tool("link"),
            "linker_type": "windows",
            "ml64": host_tool("ml64"),
            "rc": host_tool("rc"),
        }
    else:
        native_attrs = {
            "compiler_type": host_tool("compiler_type"),
            "cvtres": "",
            "cxx": host_tool("cxx"),
            "include": "",
            "lib_paths": "",
            "link": host_tool("native_link"),
            "linker_type": "darwin" if host_info().os.is_macos else "gnu",
            "ml64": "",
            "rc": "",
        }
    _host_tools_rule(
        name = name,
        ar = host_tool("ar"),
        cc = host_tool("cc"),
        clippy_driver = host_tool("clippy_driver"),
        rustc = host_tool("rustc"),
        rustdoc = host_tool("rustdoc"),
        windows = windows,
        visibility = visibility,
        **native_attrs
    )

def _host_rust_toolchain_impl(ctx):
    paths = ctx.attrs.tools[ToolPathsInfo]
    return [DefaultInfo(), RustToolchainInfo(
        allow_lints = [],
        clippy_driver = RunInfo(args = [paths.clippy_driver]),
        clippy_toml = None,
        compiler = RunInfo(args = [paths.rustc]),
        default_edition = ctx.attrs.default_edition,
        deny_lints = [],
        doctests = False,
        nightly_features = False,
        panic_runtime = PanicRuntime("unwind"),
        report_unused_deps = False,
        rustc_binary_flags = [],
        rustc_env = ctx.attrs.rustc_env,
        rustc_flags = ctx.attrs.rustc_flags,
        rustc_target_triple = ctx.attrs.rustc_target_triple,
        rustc_test_flags = [],
        rustdoc = RunInfo(args = [paths.rustdoc]),
        rustdoc_flags = [],
        warn_lints = [],
    )]

_host_rust_toolchain_rule = rule(
    impl = _host_rust_toolchain_impl,
    attrs = {
        "default_edition": attrs.string(),
        "rustc_env": attrs.dict(attrs.string(), attrs.string()),
        "rustc_flags": attrs.list(attrs.string()),
        "rustc_target_triple": attrs.string(),
        "tools": attrs.dep(providers = [ToolPathsInfo]),
    },
    is_toolchain_rule = True,
)

def host_rust_toolchain(name, default_edition, tools, visibility):
    rustc_env = {}
    if _is_windows():
        # link.exe, invoked by rustc through the wrapper, also reads LIB.
        lib_paths = host_tool("lib_paths")
        rustc_env = {
            "INCLUDE": host_tool("include"),
            "LIB": lib_paths,
            "LIBPATH": lib_paths,
        }
    elif steamos_enabled():
        # Keep Nix shell paths out of the SteamOS executable's RUNPATH.
        rustc_env = {"NIX_LDFLAGS": ""}
    _host_rust_toolchain_rule(
        name = name,
        default_edition = default_edition,
        rustc_env = rustc_env,
        rustc_flags = _rustc_flags(),
        rustc_target_triple = host_tool("rustc_target_triple"),
        tools = tools,
        visibility = visibility,
    )
