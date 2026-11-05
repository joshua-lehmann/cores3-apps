#!/usr/bin/env python3
"""Choose a stable patch tag, reusing a stable tag already on this commit."""
import re
import subprocess
import sys


def next_release(tags, current_tags):
    """Ignore non-stable tags; make retries reuse the current commit's tag."""
    def versions(names):
        return [tuple(map(int, match.groups())) for name in names
                if (match := re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", name))]
    current = versions(current_tags)
    if current:
        version = max(current)
    else:
        major, minor, patch = max(versions(tags), default=(0, 0, 0))
        version = (major, minor, patch + 1)
    return "v" + ".".join(map(str, version))


def firmware_changed(before, after, cwd=None):
    """Compare the entire push; a failed comparison must not silently skip publishing."""
    if before == "0" * 40:
        return True
    result = subprocess.run(["git", "diff", "--quiet", before, after, "--",
                             "apps", "crates", "assets", "Cargo.toml", "Cargo.lock",
                             "rust-toolchain.toml", ".cargo", "build.rs", ":(exclude)**/*.md"], cwd=cwd)
    if result.returncode not in (0, 1):
        result.check_returncode()
    return result.returncode == 1


if __name__ == "__main__" and len(sys.argv) == 4 and sys.argv[1] == "--changed":
    print(str(firmware_changed(sys.argv[2], sys.argv[3])).lower())
elif __name__ == "__main__":
    def git_tags(*args):
        return subprocess.check_output(["git", "tag", *args], text=True).splitlines()
    print(next_release(git_tags(), git_tags("--points-at", "HEAD")))
