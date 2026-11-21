load("@prelude//rust:sources.bzl", "RustSources", "RustSourcesTSet")
load("@prelude//rust:rust_toolchain.bzl", "RustToolchainInfo")
load("@toolchains//:host_toolchain.bzl", "build_mode", "buildscript_env", "steamos_linker_flags")

# Files of one package keyed by their repository-relative path, so that a
# crate's tree can hold files of other packages at their real locations.
CrateFilesInfo = provider(fields = {
    "files": provider_field(dict[str, Artifact]),
})

def _crate_files_impl(ctx: AnalysisContext) -> list[Provider]:
    prefix = ctx.label.package + "/" if ctx.label.package else ""
    files = {prefix + src.short_path: src for src in ctx.attrs.srcs}
    return [DefaultInfo(), CrateFilesInfo(files = files)]

crate_files = rule(
    impl = _crate_files_impl,
    attrs = {
        "srcs": attrs.list(attrs.source(), default = []),
    },
)

def _crate_tree_impl(ctx: AnalysisContext) -> list[Provider]:
    files = {}
    for dep in ctx.attrs.files:
        files.update(dep[CrateFilesInfo].files)

    # Real directories with symlinked files: rustc and build scripts resolve
    # `../` in include paths physically.
    tree = ctx.actions.symlinked_dir("tree", files)
    return [
        DefaultInfo(default_output = tree),
        RustSources(tset = ctx.actions.tset(RustSourcesTSet, value = tree, children = [])),
    ]

crate_tree = rule(
    impl = _crate_tree_impl,
    attrs = {
        "files": attrs.list(attrs.dep(providers = [CrateFilesInfo])),
    },
)

def _workspace_buildscript_run_impl(ctx: AnalysisContext) -> list[Provider]:
    toolchain = ctx.attrs._rust_toolchain[RustToolchainInfo]
    out_dir = ctx.actions.declare_output("OUT_DIR", dir = True)
    rustc_flags = ctx.actions.declare_output("rustc_flags")
    cmd = cmd_args(
        ctx.attrs._runner[RunInfo],
        cmd_args("--buildscript=", ctx.attrs.buildscript[RunInfo], delimiter = ""),
        cmd_args("--tree=", ctx.attrs.tree[DefaultInfo].default_outputs[0], delimiter = ""),
        "--manifest=" + ctx.attrs.manifest,
        cmd_args("--out-dir=", out_dir.as_output(), delimiter = ""),
        cmd_args("--outfile=", rustc_flags.as_output(), delimiter = ""),
        cmd_args("--rustc=", toolchain.compiler, delimiter = ""),
        "--target=" + toolchain.rustc_target_triple,
        "--package=" + ctx.attrs.package_name,
        "--version=" + ctx.attrs.version,
        "--profile=" + build_mode(),
        ["--feature=" + feature for feature in ctx.attrs.features],
        ["--env={}={}".format(key, value) for key, value in ctx.attrs.env.items()],
    )
    ctx.actions.run(cmd, category = "buildscript")
    return [DefaultInfo(
        default_output = None,
        sub_targets = {
            "out_dir": [DefaultInfo(default_output = out_dir)],
            "rustc_flags": [DefaultInfo(default_output = rustc_flags)],
        },
    )]

_workspace_buildscript_run = rule(
    impl = _workspace_buildscript_run_impl,
    attrs = {
        "buildscript": attrs.exec_dep(providers = [RunInfo]),
        "env": attrs.dict(attrs.string(), attrs.string(), default = {}),
        "features": attrs.list(attrs.string(), default = []),
        "manifest": attrs.string(),
        "package_name": attrs.string(),
        "tree": attrs.dep(),
        "version": attrs.string(),
        "_runner": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "//build-support/buck:run_buildscript")),
        "_rust_toolchain": attrs.default_only(attrs.toolchain_dep(default = "toolchains//:rust", providers = [RustToolchainInfo])),
    },
)

def _rustc_flags(dev_opt_level):
    # [profile.dev.package.<crate>] opt-level in the root Cargo.toml.
    if dev_opt_level != None and build_mode() == "debug":
        return ["-Copt-level={}".format(dev_opt_level)]
    return []

def _lto_flags():
    # Cross-crate LTO for binaries: rlibs carry bitcode by default, so only
    # the final link changes. Thin by default in release, where it cut the
    # game thread's frame time 1-3% on a Steam Machine for twice the h2 link
    # time (docs/perf/notes/steammachine-perf.md). Fat needs more memory than
    # a shared 64 GB host had free.
    lto = read_config("build_mode", "lto", "thin" if build_mode() == "release" else "off")
    if lto not in ["off", "thin", "fat"]:
        fail("[build_mode] lto must be off, thin or fat, got {}".format(lto))
    return [] if lto == "off" else ["-Clto=" + lto]

def _cargo_env(package, crate, manifest, version, description):
    numbers = version.split("-")[0].split(".")
    return {
        "CARGO_CRATE_NAME": crate,
        # Interpreted by the prelude relative to the crate tree.
        "CARGO_MANIFEST_DIR": manifest,
        "CARGO_PKG_AUTHORS": "",
        "CARGO_PKG_DESCRIPTION": description,
        "CARGO_PKG_NAME": package,
        "CARGO_PKG_REPOSITORY": "",
        "CARGO_PKG_VERSION": version,
        "CARGO_PKG_VERSION_MAJOR": numbers[0],
        "CARGO_PKG_VERSION_MINOR": numbers[1],
        "CARGO_PKG_VERSION_PATCH": numbers[2],
        "CARGO_PKG_VERSION_PRE": version.split("-")[1] if "-" in version else "",
    }

def _buildscript_args(buildscript):
    if buildscript == None:
        return {"env": {}, "rustc_flags": []}
    return {
        "env": {"OUT_DIR": "$(location {}[out_dir])".format(buildscript)},
        "rustc_flags": ["@$(location {}[rustc_flags])".format(buildscript)],
    }

def workspace_buildscript(
        name,
        package,
        version,
        manifest,
        tree,
        features = [],
        deps = [],
        named_deps = {}):
    """A first-party build.rs, compiled and run with `tree` as the source root."""
    native.rust_binary(
        name = name + "-bin",
        srcs_filegroup = tree,
        crate = "build_script_build",
        crate_root = manifest + "/build.rs",
        edition = "2024",
        env = _cargo_env(package, "build_script_build", manifest, version, ""),
        features = features,
        deps = deps,
        named_deps = named_deps,
    )
    _workspace_buildscript_run(
        name = name,
        buildscript = ":" + name + "-bin",
        # The C toolchain, for build scripts that use the cc crate.
        env = buildscript_env(),
        features = features,
        manifest = manifest,
        package_name = package,
        tree = tree,
        version = version,
    )

def workspace_library(
        name,
        package,
        crate,
        crate_root,
        version,
        manifest,
        tree,
        description = "",
        features = [],
        deps = [],
        named_deps = {},
        buildscript = None,
        dev_opt_level = None):
    script = _buildscript_args(buildscript)
    native.rust_library(
        name = name,
        srcs_filegroup = tree,
        crate = crate,
        crate_root = crate_root,
        edition = "2024",
        env = _cargo_env(package, crate, manifest, version, description) | script["env"],
        features = features,
        rustc_flags = _rustc_flags(dev_opt_level) + script["rustc_flags"],
        deps = deps,
        named_deps = named_deps,
        visibility = ["PUBLIC"],
    )

def workspace_binary(
        name,
        package,
        crate,
        crate_root,
        version,
        manifest,
        tree,
        description = "",
        features = [],
        deps = [],
        named_deps = {},
        buildscript = None,
        dev_opt_level = None,
        heap_profile_feature = None):
    """`heap_profile_feature` (dhat) is added under `-c build_mode.dhat=true`."""
    script = _buildscript_args(buildscript)
    if heap_profile_feature != None and read_config("build_mode", "dhat", "false") == "true":
        features = features + [heap_profile_feature]
    native.rust_binary(
        name = name,
        srcs_filegroup = tree,
        crate = crate,
        crate_root = crate_root,
        edition = "2024",
        env = _cargo_env(package, crate, manifest, version, description) | {"CARGO_BIN_NAME": name} | script["env"],
        features = features,
        linker_flags = steamos_linker_flags(),
        rustc_flags = _rustc_flags(dev_opt_level) + _lto_flags() + script["rustc_flags"],
        deps = deps,
        named_deps = named_deps,
        visibility = ["PUBLIC"],
    )

def workspace_test(
        name,
        package,
        crate,
        crate_root,
        version,
        manifest,
        tree,
        description = "",
        features = [],
        deps = [],
        named_deps = {},
        buildscript = None,
        dev_opt_level = None,
        binaries = {}):
    """`binaries` maps a binary name to its target for CARGO_BIN_EXE_<name>."""
    script = _buildscript_args(buildscript)
    env = _cargo_env(package, crate, manifest, version, description) | script["env"] | {
        "CARGO_TARGET_TMPDIR": "/tmp",
        # Tests run from the project root, so insta finds snapshots in the
        # source tree.
        "INSTA_WORKSPACE_ROOT": ".",
    }
    for binary, target in binaries.items():
        env["CARGO_BIN_EXE_" + binary] = "$(location {})".format(target)
    native.rust_test(
        name = name,
        srcs_filegroup = tree,
        crate = crate,
        crate_root = crate_root,
        edition = "2024",
        env = env,
        features = features,
        rustc_flags = _rustc_flags(dev_opt_level) + script["rustc_flags"],
        deps = deps,
        named_deps = named_deps,
        visibility = ["PUBLIC"],
    )
