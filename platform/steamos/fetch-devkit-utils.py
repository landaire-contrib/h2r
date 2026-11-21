"""Download Valve's steamos-devkit at a pinned commit and extract devkit-utils.

GitLab answers HEAD with Content-Length 0, which Buck's http_archive rejects,
so the archive is fetched here and checked against its SHA-256.
Usage: fetch-devkit-utils.py <url> <sha256> <output directory>
"""

import hashlib
import io
import os
import sys
import tarfile
import urllib.request


def main():
    url, expected, out = sys.argv[1:4]
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read()
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        sys.exit("{}: SHA-256 {} does not match the pinned {}".format(url, actual, expected))

    marker = "/client/devkit-utils/"
    os.makedirs(out)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive.getmembers():
            if marker not in member.name or not (member.isfile() or member.isdir()):
                continue
            relative = member.name.split(marker, 1)[1]
            if not relative:
                continue
            target = os.path.join(out, relative)
            if member.isdir():
                os.makedirs(target, exist_ok=True)
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "wb") as f:
                f.write(archive.extractfile(member).read())
            os.chmod(target, member.mode & 0o755)
    return 0


if __name__ == "__main__":
    sys.exit(main())
