"""Write the [host_toolchain] section of .buckconfig.local.

Buck2 actions receive every compiler, linker and interpreter as an absolute path
from this section rather than resolving them through PATH. Run it with the
Python interpreter that the prelude's internal tools should use; on Windows
that must be a native CPython (for example `py -3`), not an MSYS2 build.

Overrides: CC, CXX and AR select the C toolchain on Linux and macOS.
"""

import os
import re
import shutil
import subprocess
import sys
import sysconfig

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCAL_CONFIG = os.path.join(REPO, ".buckconfig.local")
SECTION = "host_toolchain"


class ConfigureError(Exception):
    pass


def run(args):
    return subprocess.run(args, cwd=REPO, check=True, capture_output=True, text=True).stdout


def require_file(path, description):
    if not os.path.isfile(path):
        raise ConfigureError("{} not found: {}".format(description, path))
    return path


def python_interpreter():
    if os.name == "nt" and (
        sysconfig.get_platform().startswith("mingw") or "msys" in sys.version.lower()
    ):
        raise ConfigureError(
            "{} is an MSYS2 Python. Run this script with a native CPython, "
            "for example `py -3 scripts/configure-buck-toolchain.py`.".format(sys.executable)
        )
    return sys.executable


def rust_tools():
    # rustup resolves rust-toolchain.toml relative to the working directory.
    if shutil.which("rustup"):
        rustc = run(["rustup", "which", "rustc"]).strip()
    else:
        rustc = shutil.which("rustc")
        if rustc is None:
            raise ConfigureError("rustc not found on PATH")
    bin_dir = os.path.dirname(rustc)
    exe = ".exe" if os.name == "nt" else ""
    host = re.search(r"^host: (\S+)$", run([rustc, "-vV"]), re.M)
    if host is None:
        raise ConfigureError("`rustc -vV` reported no host triple")
    return {
        "rustc": rustc,
        "rustdoc": require_file(os.path.join(bin_dir, "rustdoc" + exe), "rustdoc"),
        "clippy_driver": require_file(os.path.join(bin_dir, "clippy-driver" + exe), "clippy-driver"),
        "rustc_target_triple": host.group(1),
    }


def msvc_tools():
    vswhere = os.path.join(
        os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        "Microsoft Visual Studio",
        "Installer",
        "vswhere.exe",
    )
    require_file(vswhere, "vswhere (Visual Studio Installer)")
    install = run(
        [
            vswhere,
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ]
    ).strip()
    if not install:
        raise ConfigureError("no Visual Studio installation with the MSVC x64 tools found")

    version_file = os.path.join(install, "VC", "Auxiliary", "Build", "Microsoft.VCToolsVersion.default.txt")
    with open(require_file(version_file, "MSVC version file"), encoding="utf-8") as f:
        msvc_version = f.read().strip()
    msvc = os.path.join(install, "VC", "Tools", "MSVC", msvc_version)
    msvc_bin = os.path.join(msvc, "bin", "Hostx64", "x64")

    import winreg

    with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows Kits\Installed Roots") as key:
        kits = winreg.QueryValueEx(key, "KitsRoot10")[0]

    def complete(version):
        return all(
            os.path.isfile(p)
            for p in [
                os.path.join(kits, "Include", version, "um", "Windows.h"),
                os.path.join(kits, "Lib", version, "um", "x64", "kernel32.lib"),
                os.path.join(kits, "bin", version, "x64", "rc.exe"),
            ]
        )

    versions = [v for v in os.listdir(os.path.join(kits, "Include")) if v.startswith("10.") and complete(v)]
    if not versions:
        raise ConfigureError("no complete Windows 10/11 SDK found under " + kits)
    sdk = max(versions, key=lambda v: tuple(int(p) for p in v.split(".")))

    tool = lambda name: require_file(os.path.join(msvc_bin, name), name)
    include = ";".join(
        [os.path.join(msvc, "include")]
        + [os.path.join(kits, "Include", sdk, d) for d in ["ucrt", "um", "shared"]]
    )
    lib_paths = ";".join(
        [os.path.join(msvc, "lib", "x64")]
        + [os.path.join(kits, "Lib", sdk, d, "x64") for d in ["ucrt", "um"]]
    )
    return {
        "ar": tool("lib.exe"),
        "cc": tool("cl.exe"),
        "cxx": tool("cl.exe"),
        "link": tool("link.exe"),
        "ml64": tool("ml64.exe"),
        "cvtres": tool("cvtres.exe"),
        "rc": require_file(os.path.join(kits, "bin", sdk, "x64", "rc.exe"), "rc.exe"),
        "include": include,
        "lib_paths": lib_paths,
    }


def unix_tools():
    def find(var, candidates):
        names = [os.environ[var]] if os.environ.get(var) else candidates
        for name in names:
            path = shutil.which(name)
            if path:
                return os.path.abspath(path)
        raise ConfigureError("none of {} found on PATH (set {})".format(", ".join(names), var))

    cc = find("CC", ["clang", "cc", "gcc"])
    cxx = find("CXX", ["clang++", "c++", "g++"])
    ar = find("AR", ["llvm-ar", "ar"])
    compiler_type = "clang" if "clang" in run([cc, "--version"]).lower() else "gcc"
    if sys.platform.startswith("linux") and shutil.which("ld.lld") is None:
        # The prelude's C++ toolchain links with -fuse-ld=lld on Linux.
        raise ConfigureError("ld.lld not found on PATH; install lld")
    return {
        "ar": ar,
        "cc": cc,
        "cxx": cxx,
        "native_link": cxx,
        "compiler_type": compiler_type,
    }


def write_section(values):
    existing = ""
    if os.path.exists(LOCAL_CONFIG):
        with open(LOCAL_CONFIG, encoding="utf-8") as f:
            existing = f.read()
    # Drop a previous [host_toolchain] section and keep everything else.
    kept, skipping = [], False
    for line in existing.splitlines():
        header = re.match(r"^\s*\[([^\]]+)\]\s*$", line)
        if header:
            skipping = header.group(1).strip() == SECTION
        if not skipping:
            kept.append(line)
    body = "\n".join(kept).strip()
    section = "[{}]\n".format(SECTION) + "".join(
        "  {} = {}\n".format(key, values[key]) for key in sorted(values)
    )
    text = (body + "\n\n" if body else "") + section
    # No BOM: Buck2 rejects one.
    with open(LOCAL_CONFIG, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def main():
    try:
        values = {"python": python_interpreter()}
        values.update(rust_tools())
        values.update(msvc_tools() if os.name == "nt" else unix_tools())
    except (ConfigureError, subprocess.CalledProcessError, OSError) as err:
        sys.stderr.write("error: {}\n".format(err))
        return 1
    write_section(values)
    print("wrote [{}] to {}".format(SECTION, LOCAL_CONFIG))
    return 0


if __name__ == "__main__":
    sys.exit(main())
