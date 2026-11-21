"""Copy the SteamOS package and the disc to a Steam Machine and launch it.

Uses the SteamOS devkit service that Developer Mode enables, as Valve's
devkit client does: the same pairing key (~/.config/steamos-devkit/devkit_rsa),
the devkit-utils scripts on the device, and a Steam shortcut launched in the
Steam Linux Runtime 4 container. Run through `buck2 run //platform/steamos:run`.
"""

import argparse
import json
import os
import shlex
import subprocess
import sys
import time
import urllib.error
import urllib.request

GAME_ID = "h2r"
DEVKIT_PORT = 32000

# Sent with the key when pairing; the devkit service rejects keys without it.
DEVKIT_MAGIC_PHRASE = "900b919520e4cf601998a71eec318fec"

KEY_DIR = os.path.expanduser("~/.config/steamos-devkit")
KEY = os.path.join(KEY_DIR, "devkit_rsa")


class DeployError(Exception):
    pass


def log(message):
    print("steamos: " + message, file=sys.stderr, flush=True)


def discover_device():
    """IPv4 address of the first SteamOS devkit service on the local network."""
    try:
        output = subprocess.run(
            ["avahi-browse", "--resolve", "--terminate", "--parsable", "_steamos-devkit._tcp"],
            check=True,
            capture_output=True,
            text=True,
            timeout=15,
        ).stdout
    except (OSError, subprocess.SubprocessError) as err:
        raise DeployError("no device given and mDNS discovery failed ({}); set [steamos] device".format(err))
    for line in output.splitlines():
        fields = line.split(";")
        if fields[0] == "=" and fields[2] == "IPv4":
            log("found {} at {}".format(fields[3], fields[7]))
            return fields[7]
    raise DeployError("no SteamOS devkit service found on the network; set [steamos] device")


def resolve(device):
    """An address for `device`. Nix's glibc has no mDNS resolver, so `.local`
    names go through avahi."""
    if not device.endswith(".local"):
        return device
    try:
        output = subprocess.run(
            ["avahi-resolve", "-4", "--name", device], check=True, capture_output=True, text=True, timeout=15
        ).stdout.split()
    except (OSError, subprocess.SubprocessError) as err:
        raise DeployError("cannot resolve {} over mDNS: {}".format(device, err))
    if len(output) != 2:
        raise DeployError("cannot resolve {} over mDNS".format(device))
    return output[1]


def http(device, path, data=None, timeout=10):
    request = urllib.request.Request(
        "http://{}:{}/{}".format(device, DEVKIT_PORT, path),
        data=data,
        headers={"Content-Type": "text/plain"} if data is not None else {},
        method="POST" if data is not None else "GET",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.read().decode("utf-8").strip()


def ensure_key():
    if os.path.exists(KEY):
        return
    os.makedirs(KEY_DIR, exist_ok=True)
    subprocess.run(["ssh-keygen", "-q", "-t", "rsa", "-b", "4096", "-N", "", "-f", KEY, "-C", "h2r-devkit"], check=True)


def ssh_command(login, device):
    # No config file: the host's system ssh_config can carry options this
    # ssh does not know, and every option it needs is given here.
    return [
        "ssh",
        "-F",
        "/dev/null",
        "-i",
        KEY,
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "{}@{}".format(login, device),
    ]


def ssh(login, device, command, capture=False, stdin=None):
    result = subprocess.run(
        ssh_command(login, device) + [command],
        check=False,
        capture_output=capture,
        input=stdin,
        stdin=subprocess.DEVNULL if stdin is None else None,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() if capture else ""
        raise DeployError("remote command failed ({}): {} {}".format(result.returncode, command, detail))
    return result.stdout if capture else ""


def can_ssh(login, device):
    return subprocess.run(ssh_command(login, device) + ["true"], capture_output=True).returncode == 0


def pair(login, device):
    if can_ssh(login, device):
        return
    with open(KEY + ".pub", encoding="utf-8") as f:
        key = f.read().strip()
    log("pairing: open Settings > Developer > Pair new host on the device, then approve the request")
    try:
        reply = http(device, "register", (key + " " + DEVKIT_MAGIC_PHRASE + "\n").encode("ascii"), timeout=120)
    except urllib.error.HTTPError as err:
        reply = err.read().decode("utf-8", "replace").strip()
    except urllib.error.URLError as err:
        raise DeployError("pairing request failed: {}".format(err))
    try:
        refusal = json.loads(reply).get("error")
    except (ValueError, AttributeError):
        refusal = None
    if refusal:
        raise DeployError("the device refused pairing: {}".format(refusal.strip()))
    log("devkit service replied: {}".format(reply))
    for _ in range(30):
        if can_ssh(login, device):
            return
        time.sleep(2)
    raise DeployError("paired, but SSH to {}@{} still fails".format(login, device))


def rsync(login, device, source, destination, extra=()):
    command = [
        "rsync",
        "--archive",
        "--partial",
        "--human-readable",
        "--info=progress2",
        "-e",
        shlex.join(ssh_command(login, device)[:-1]),
        *extra,
        source,
        "{}@{}:{}".format(login, device, destination),
    ]
    if subprocess.run(command).returncode != 0:
        raise DeployError("rsync of {} failed".format(source))


def deploy(args):
    device = resolve(args.device) if args.device else discover_device()
    ensure_key()
    try:
        login = http(device, "login-name")
    except urllib.error.URLError as err:
        raise DeployError("no devkit service at {}:{} ({}); is Developer Mode on?".format(device, DEVKIT_PORT, err))
    pair(login, device)

    rsync(login, device, os.path.join(args.devkit_utils, ""), "devkit-utils/", ["--delete"])
    prepared = json.loads(ssh(login, device, "python3 ~/devkit-utils/steamos-prepare-upload --gameid " + GAME_ID, True))
    directory = prepared["directory"]

    # Steam's run-game does nothing while the shortcut is running, and the
    # running game holds the old module open. The bracket keeps the pattern
    # from matching this command's own shell.
    ssh(login, device, "pkill -f {} || true".format(shlex.quote(directory + "/h2 pla[y]")))

    log("copying the package to {}".format(directory))
    rsync(login, device, os.path.join(args.package, ""), directory + "/", ["--delete", "--copy-links", "--filter=P /disc/***"])
    log("copying the disc (only the first run sends all of it)")
    ssh(login, device, "mkdir -p {}/disc".format(shlex.quote(directory)))
    rsync(login, device, args.disc, directory + "/disc/halo2.iso", ["--copy-links"])

    # Steam does not pass a shortcut's environment into the runtime
    # container, so launch.sh exports the lines of h2-env.
    env_lines = "".join("{}={}\n".format(key, value) for key, value in args.env)
    ssh(login, device, "cat > {}".format(shlex.quote(directory + "/h2-env")), stdin=env_lines)

    parameters = {
        "gameid": GAME_ID,
        "directory": directory,
        "argv": ["./launch.sh"] + args.game_args,
        "env": {},
        "settings": {"steam_play": "0", "compat_tool": "SteamLinuxRuntime_4"},
        "force_appid": None,
        "lepton_args": None,
    }
    created = json.loads(
        ssh(
            login,
            device,
            "python3 ~/devkit-utils/steam-client-create-shortcut --parms " + shlex.quote(json.dumps(parameters)),
            True,
        )
    )
    if "error" in created:
        raise DeployError("Steam shortcut: {}".format(created["error"]))

    if args.no_launch:
        return
    log("launching")
    ssh(login, device, "python3 ~/devkit-utils/steam-devkit-rpc run-game gameid=" + GAME_ID)


def env_entry(value):
    key, separator, setting = value.partition("=")
    if not key or not separator:
        raise argparse.ArgumentTypeError("expected KEY=VALUE, got {!r}".format(value))
    return key, setting


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", required=True, help="the built SteamOS package directory")
    parser.add_argument("--disc", required=True, help="the Halo 2 disc image")
    parser.add_argument("--devkit-utils", required=True, help="Valve's devkit-utils directory")
    parser.add_argument("--device", default="", help="host name or address; discovered over mDNS when empty")
    parser.add_argument("--no-launch", action="store_true", help="copy and register without launching")
    parser.add_argument("--env", action="append", default=[], type=env_entry, metavar="KEY=VALUE", help="environment for the game")
    # Every argument this parser does not take goes to `h2 play`.
    args, game_args = parser.parse_known_args()
    args.game_args = [arg for arg in game_args if arg != "--"]
    try:
        deploy(args)
    except DeployError as err:
        log("error: {}".format(err))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
