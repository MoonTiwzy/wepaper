#!/bin/sh
# Runs when a wallpaper is shown. The daemon says what happened in WEPAPER_* variables:
#   WEPAPER_EVENT      shown | hidden | paused | resumed | started | stopping
#   WEPAPER_OUTPUT     the output's name, e.g. DP-1
#   WEPAPER_WALLPAPER  the wallpaper's folder
#   WEPAPER_TITLE      its title
#   WEPAPER_KIND       scene | video | web
#   WEPAPER_SOCKET     the daemon's control socket, for `wepaper --socket ...`
log="${XDG_STATE_HOME:-$HOME/.local/state}/wepaper/shown.log"
mkdir -p "$(dirname "$log")"
printf '%s %s %s (%s)\n' "$(date +%FT%T)" "$WEPAPER_OUTPUT" "$WEPAPER_TITLE" "$WEPAPER_WALLPAPER" >> "$log"
