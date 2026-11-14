#!/usr/bin/env bash
# A compositor nobody sees, to run the daemon in: a headless host with a Hyprland nested
# in it. Nothing reaches the live session.
#
#   tools/bed.sh up [render node] [WxH]   start it (default /dev/dri/renderD128, 2560x1440)
#   tools/bed.sh run <command...>         run a command inside it
#   tools/bed.sh shot <file.png> [output] a screenshot of an output (all of them without a name);
#                                         gives up after ten seconds: an output nothing draws on
#                                         never delivers a frame
#   tools/bed.sh down                     stop it
#
# Needs the headless host of ~/.cache/noshare-cover/bed (a cage that speaks what a
# nested Hyprland asks for); WEPAPER_BED names another runtime directory.
set -uo pipefail
host=${WEPAPER_BED:-/tmp/wepaper-bed}
nest=$host/nest
bed=$HOME/.cache/noshare-cover/bed/bed.sh
inside() {
    local signature
    signature=$(/usr/bin/ls "$nest/hypr" 2>/dev/null | head -1)
    env -u DISPLAY XDG_RUNTIME_DIR="$nest" WAYLAND_DISPLAY=wayland-1 HYPRLAND_INSTANCE_SIGNATURE="$signature" "$@"
}
case ${1:-} in
up)
    NSC_BED_RT=$host NSC_BED_SIZE=${3:-2560x1440} "$bed" up "${2:-/dev/dri/renderD128}" || exit 1
    mkdir -p -m 700 "$nest"
    printf 'hl.config({ xwayland = { enabled = false }, misc = { disable_watchdog_warning = true, disable_hyprland_logo = true, disable_splash_rendering = true }, animations = { enabled = false } })\n' >"$host/hyprland.lua"
    env -u HYPRLAND_INSTANCE_SIGNATURE -u DISPLAY XDG_RUNTIME_DIR="$nest" WAYLAND_DISPLAY="$host/wayland-0" \
        HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1 setsid /usr/bin/Hyprland --config "$host/hyprland.lua" >"$host/hyprland.log" 2>&1 &
    echo $! >"$host/hyprland.pid"
    for _ in $(seq 100); do [[ -S $nest/wayland-1 ]] && break; sleep 0.1; done
    [[ -S $nest/wayland-1 ]] && echo "nest up: $nest/wayland-1" || { echo "the nested Hyprland did not start"; tail -5 "$host/hyprland.log"; exit 1; }
    ;;
run) shift; inside "$@" ;;
shot) inside timeout 10 grim ${3:+-o "$3"} "${2:?file}" ;;
down)
    [[ -f $host/hyprland.pid ]] && kill "$(<"$host/hyprland.pid")" 2>/dev/null
    sleep 0.3
    NSC_BED_RT=$host "$bed" down
    ;;
*) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
