#!/bin/sh
# Exit-criteria checks for melibea-ruler in a nested niri.
#
# usage: ruler/tests/nest.sh SCALE
#
# Starts a nested niri at the given output scale, opens the probe window, finds the window's
# centre over niri's IPC, and measures there with the headless mode. Prints the measurement and
# PASS or FAIL. The nested window briefly takes focus on the host session.
set -eu

scale=${1:?usage: nest.sh SCALE}
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ruler=${RULER:-$here/../../target/debug/melibea-ruler}
work=$(mktemp -d)
niri_pid=
probe_pid=
trap 'kill "$probe_pid" "$niri_pid" 2>/dev/null || true; rm -rf "$work"' EXIT

cat > "$work/niri.kdl" <<EOK
output "winit" { scale $scale; }
layout {
    gaps 0
    focus-ring { off; }
    border { off; }
    default-column-width { proportion 1.0; }
}
prefer-no-csd
EOK
niri validate -c "$work/niri.kdl" > /dev/null 2>&1

niri -c "$work/niri.kdl" > "$work/niri.log" 2>&1 &
niri_pid=$!
display=
socket=
for _ in $(seq 1 100); do
    display=$(grep -oE 'Wayland socket: wayland-[0-9]+' "$work/niri.log" | grep -oE 'wayland-[0-9]+' | head -n 1 || true)
    socket=$(grep -oE '/run/user/[0-9]+/niri\.wayland-[0-9]+\.[0-9]+\.sock' "$work/niri.log" | head -n 1 || true)
    [ -n "$display" ] && [ -n "$socket" ] && break
    sleep 0.2
done
[ -n "$display" ] && [ -n "$socket" ] || { echo "nested niri did not start" >&2; cat "$work/niri.log" >&2; exit 1; }

WAYLAND_DISPLAY=$display GDK_BACKEND=wayland python3 "$here/probes/box.py" &
probe_pid=$!

centre=
for _ in $(seq 1 100); do
    centre=$(NIRI_SOCKET=$socket niri msg --json windows 2>/dev/null | python3 -c '
import json, sys
try:
    found = [w for w in json.load(sys.stdin) if w["title"] == "RULER-PROBE"]
    w = found[0]["layout"]
    # niri reports no tile position for a lone tiled window; with the column at the full
    # width of the output and no gaps or bars, it sits at the origin.
    tx, ty = w["tile_pos_in_workspace_view"] or (0, 0)
    ox, oy = w["window_offset_in_tile"]
    ww, wh = w["window_size"]
    print(f"{tx + ox + ww / 2},{ty + oy + wh / 2}")
except (ValueError, IndexError, KeyError, TypeError):
    pass
' || true)
    [ -n "$centre" ] && break
    sleep 0.2
done
[ -n "$centre" ] || { echo "probe window not found" >&2; exit 1; }
# Let the window finish its first frame.
sleep 1

for mode in box spacing; do
    result=$(WAYLAND_DISPLAY=$display "$ruler" --measure "$mode" --at "$centre" --output winit)
    if [ "$result" = 300x200 ]; then verdict=PASS; else verdict=FAIL; fi
    echo "scale $scale $mode at $centre: $result $verdict"
done
