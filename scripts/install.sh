#!/bin/sh
set -eu

# install.sh — build Melibea in release, install it, then drop the build tree.
#
# The binary goes to $MELIBEA_PREFIX/bin/melibea (default ~/.local), the path
# contrib/systemd/melibea.service runs. It is copied to a temporary file and
# renamed over the old one, so a running daemon keeps its open binary and picks
# up the new one at its next restart; this script never restarts it.
#
# Once the installed bytes are checked against the build, target/ holds only a
# cache that the next build regenerates, so it is removed: debug and release
# trees together reach gigabytes and nothing reads them after the install.
# Pass --keep-target to leave it, for example while iterating.

usage() {
    echo "usage: scripts/install.sh [--keep-target]" >&2
    exit 2
}

keep_target=0
case $# in
    0) ;;
    1) [ "$1" = --keep-target ] || usage; keep_target=1 ;;
    *) usage ;;
esac

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
prefix=${MELIBEA_PREFIX:-$HOME/.local}
destination=$prefix/bin/melibea

(cd "$repo_root" && cargo build --release --locked)
built=$repo_root/target/release/melibea

mkdir -p -- "$prefix/bin"
temporary=$(mktemp "$prefix/bin/.melibea.XXXXXX")
trap 'rm -f -- "$temporary"' EXIT HUP INT TERM
install -m 0755 "$built" "$temporary"
mv -f -- "$temporary" "$destination"
trap - EXIT HUP INT TERM
cmp -s -- "$built" "$destination" || {
    echo "install: $destination does not match $built" >&2
    exit 1
}
echo ">> installed $destination" >&2

if [ "$keep_target" -eq 0 ]; then
    (cd "$repo_root" && cargo clean --quiet)
    echo ">> removed target/; the next build regenerates it" >&2
fi
if command -v systemctl >/dev/null 2>&1 \
    && systemctl --user is-active --quiet melibea.service 2>/dev/null; then
    echo "   melibea.service is running the previous binary until it restarts" >&2
fi
