#!/usr/bin/env bash
# What a wallpaper costs while it plays, measured in the hidden compositor of tools/bed.sh.
#
#   tools/footprint.sh <label> <out.tsv> <wallpaper>... -- <command that shows "$WALLPAPER">
#
# For each wallpaper the command is started fresh inside the bed (with WALLPAPER set to the
# folder), left to settle, and its process is measured for a while:
#   load_ms   until its log says it plays (a line containing "playing")
#   vram_mb   video memory of the process (nvidia-smi)
#   pss_mb    proportional set size
#   cpu_pct   CPU time over the window, in percent of one core
#   threads
# The command's output is its log. One line per wallpaper is appended to <out.tsv>.
set -uo pipefail
label=$1 out=$2; shift 2
wallpapers=()
while (($#)) && [[ $1 != -- ]]; do wallpapers+=("$1"); shift; done
shift
here=$(cd "$(dirname "$0")" && pwd)
settle=${SETTLE:-6} window=${WINDOW:-10}
log=$(mktemp)
[[ -s $out ]] || printf 'engine\twallpaper\tload_ms\tvram_mb\tpss_mb\tcpu_pct\tthreads\n' >"$out"
for wallpaper in "${wallpapers[@]}"; do
    : >"$log"
    start=$(date +%s%N)
    WALLPAPER=$wallpaper "$here/bed.sh" run "$@" >"$log" 2>&1 &
    load=-
    for _ in $(seq 400); do
        if /usr/bin/grep -q 'playing' "$log"; then load=$(( ($(date +%s%N) - start) / 1000000 )); break; fi
        sleep 0.02
    done
    sleep "$settle"
    # The process that holds video memory and whose environment names this run.
    pid=""
    for candidate in $(nvidia-smi --query-compute-apps=pid --format=csv,noheader 2>/dev/null; nvidia-smi | awk '$5 ~ /^[0-9]+$/ && $6 == "G" {print $5}'); do
        tr '\0' '\n' <"/proc/$candidate/environ" 2>/dev/null | /usr/bin/grep -qx "WALLPAPER=$wallpaper" && pid=$candidate
    done
    if [[ -z $pid ]]; then
        printf '%s\t%s\t%s\t-\t-\t-\t-\n' "$label" "$(basename "$wallpaper")" "$load" >>"$out"
    else
        ticks0=$(awk '{print $14 + $15}' "/proc/$pid/stat")
        sleep "$window"
        ticks1=$(awk '{print $14 + $15}' "/proc/$pid/stat")
        cpu=$(awk -v d=$((ticks1 - ticks0)) -v hz="$(getconf CLK_TCK)" -v w="$window" 'BEGIN {printf "%.1f", d * 100 / hz / w}')
        vram=$(nvidia-smi | awk -v pid="$pid" '$5 == pid {gsub("MiB", "", $(NF-1)); print $(NF-1)}' | head -1)
        pss=$(awk '/^Pss:/ {s += $2} END {printf "%.0f", s / 1024}' "/proc/$pid/smaps" 2>/dev/null)
        threads=$(/usr/bin/ls "/proc/$pid/task" 2>/dev/null | wc -l)
        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$(basename "$wallpaper")" "$load" "${vram:--}" "${pss:--}" "$cpu" "$threads" >>"$out"
    fi
    # Everything of this run goes, whatever it started.
    for p in $(pgrep -u "$(id -u)" . 2>/dev/null); do
        tr '\0' '\n' <"/proc/$p/environ" 2>/dev/null | /usr/bin/grep -qx "WALLPAPER=$wallpaper" && kill "$p" 2>/dev/null
    done
    sleep 1
done
rm -f "$log"
tail -n "${#wallpapers[@]}" "$out"
