#!/usr/bin/env bash
# End-to-end test of wepaper-web: the plugin on a real compositor, driven over its
# pipes the way the daemon drives it, showing the page in wallpaper/.
#
#   web/tests/e2e/run.sh <wepaper-web binary> [output name]
#
# Run it inside a nested or headless compositor only: XDG_RUNTIME_DIR and
# WAYLAND_DISPLAY must be that compositor's (the script refuses the session's own
# runtime directory, where it would cover the desktop). With Hyprland's
# HYPRLAND_INSTANCE_SIGNATURE set too, the layer checks run as well; with wlrctl
# installed (a virtual pointer; a headless compositor has no other), the pointer checks.
#
# Requires: grim, imagemagick; optional: hyprctl and jq, wlrctl. Exit code 0 means all passed.
set -uo pipefail

BIN=$(realpath "${1:?usage: run.sh <wepaper-web binary> [output name]}")
HERE=$(cd "$(dirname "$0")" && pwd)
PAGE=$HERE/wallpaper
WORK=$(mktemp -d "${TMPDIR:-/tmp}/wepaper-web-e2e.XXXXXX")
FAILS=0

[[ -n ${WAYLAND_DISPLAY:-} && -n ${XDG_RUNTIME_DIR:-} ]] || { echo "no WAYLAND_DISPLAY / XDG_RUNTIME_DIR" >&2; exit 2; }
if [[ $(realpath "$XDG_RUNTIME_DIR") == "/run/user/$(id -u)" ]]; then
    echo "XDG_RUNTIME_DIR is the session's own: this test draws on an output, run it in a nested compositor" >&2
    exit 2
fi

have_hyprctl=0
command -v hyprctl >/dev/null && [[ -n ${HYPRLAND_INSTANCE_SIGNATURE:-} ]] && hyprctl version >/dev/null 2>&1 && have_hyprctl=1
OUTPUT=${2:-}
if [[ -z $OUTPUT && $have_hyprctl == 1 ]]; then
    OUTPUT=$(hyprctl monitors -j | jq -r '.[0].name')
fi
[[ -n $OUTPUT ]] || { echo "name the output: run.sh <binary> <output>" >&2; exit 2; }

pass() { printf '  \e[32mok\e[0m   %s\n' "$*"; }
fail() { printf '  \e[31mFAIL\e[0m %s\n' "$*"; FAILS=$((FAILS + 1)); }
check() { local name=$1; shift; if "$@"; then pass "$name"; else fail "$name"; fi; }

PID=""
# start <args...>: the plugin with a pipe on stdin (fd 3 here), stdout and stderr in files
start() {
    rm -f "$WORK/in"; mkfifo "$WORK/in"
    # the logs of an earlier start stay, for whoever looks into a failure
    [[ -s $WORK/err ]] && mv "$WORK/err" "$WORK/err.earlier" && mv "$WORK/out" "$WORK/out.earlier"
    : >"$WORK/out"; : >"$WORK/err"
    "$BIN" "$@" <"$WORK/in" >"$WORK/out" 2>"$WORK/err" &
    PID=$!
    exec 3>"$WORK/in"
}
send() { echo "$1" >&3; }
# The lines the page and the plugin logged since `mark`.
mark() { MARK=$(wc -l <"$WORK/err"); }
since() { tail -n "+$((MARK + 1))" "$WORK/err"; }
# wait_log <fixed string> [seconds]: a log line since the mark contains it
wait_log() {
    local i
    for ((i = 0; i < ${2:-8} * 10; i++)); do
        since | grep -qF -- "$1" && return 0
        sleep 0.1
    done
    return 1
}
wait_out() {
    local i
    for ((i = 0; i < ${2:-8} * 10; i++)); do
        grep -qF -- "$1" "$WORK/out" && return 0
        sleep 0.1
    done
    return 1
}
# wait_exit <seconds>: the plugin's exit code in CODE, 255 if it had to be killed
wait_exit() {
    local i
    for ((i = 0; i < $1 * 10; i++)); do
        kill -0 "$PID" 2>/dev/null || { wait "$PID"; CODE=$?; return 0; }
        sleep 0.1
    done
    kill -9 "$PID" 2>/dev/null; wait "$PID" 2>/dev/null; CODE=255
}
# The last "second" report of the page, as its fields.
field() { since | grep -F 'e2e second' | tail -1 | grep -oE "$1=[^ ]+" | cut -d= -f2; }
fresh_second() { mark; wait_log 'e2e second' 4; mark; wait_log 'e2e second' 4; }
shot() { grim "$WORK/$1.png"; }
# The page's background as "r g b", 0 to 255: sampled off-centre, where the pointer is not.
background() { magick "$WORK/$1.png" -gravity center -crop 40x40+300+200 +repage -resize 1x1\! -format '%[fx:int(255*r+.5)] %[fx:int(255*g+.5)] %[fx:int(255*b+.5)]' info:; }
have_pointer=0
command -v wlrctl >/dev/null && have_pointer=1
skip() { printf '  \e[33mskip\e[0m %s\n' "$*"; }
# Every process below $1.
descendants() {
    local child
    for child in $(pgrep -P "$1"); do echo "$child"; descendants "$child"; done
}
between() { [[ -n $1 ]] && (($1 >= $2 && $1 <= $3)); }
cleanup() {
    exec 3>&- 2>/dev/null
    [[ -n $PID ]] && kill "$PID" 2>/dev/null
    [[ $FAILS == 0 ]] && rm -rf "$WORK" || echo "logs kept in $WORK"
}
trap cleanup EXIT

echo "command line and start-up failures"
"$BIN" >"$WORK/out" 2>"$WORK/err" </dev/null; CODE=$?
check "no arguments: exit 1 and an error event" test "$CODE" = 1 -a "$(grep -c '"event":"error"' "$WORK/out")" = 1
"$BIN" --dir "$WORK/nowhere" --output "$OUTPUT" >"$WORK/out" 2>"$WORK/err" </dev/null; CODE=$?
check "no such wallpaper: exit 1 and an error event" test "$CODE" = 1 -a "$(grep -c '"event":"error"' "$WORK/out")" = 1
"$BIN" --dir "$PAGE" --output NO-SUCH-1 >"$WORK/out" 2>"$WORK/err" </dev/null; CODE=$?
check "no such output: exit 2" test "$CODE" = 2
check "no such output: the error names the outputs there are" grep -qF "$OUTPUT" "$WORK/out"
check "log lines are 'wepaper-web: <level>: <message>'" grep -qE '^wepaper-web: error: no output named NO-SUCH-1' "$WORK/err"

echo "start"
MARK=0
start --dir "$PAGE" --output "$OUTPUT" --namespace wepaper-e2e --fps 30 --volume 50 \
    --property speed=7 --property 'motto="say \"hi\" 右 </script>"'
check "ready event" wait_out '{"event":"ready"}' 20
if [[ $have_hyprctl == 1 ]]; then
    layer=$(hyprctl layers -j | jq -c --arg o "$OUTPUT" '.[$o].levels["0"][] | select(.namespace == "wepaper-e2e")')
    check "a background layer with the namespace, owned by the plugin" test "$(jq -r .pid <<<"$layer")" = "$PID"
    mode=$(hyprctl monitors -j | jq -r --arg o "$OUTPUT" '.[] | select(.name == $o) | "\(.width / .scale | floor) \(.height / .scale | floor)"')
    check "the layer covers the whole output" test "$(jq -r '"\(.w) \(.h)"' <<<"$layer")" = "$mode"
fi

echo "the property listener"
check "applyUserProperties after load, with a listener assigned in onload" wait_log 'e2e user {'
first=$(since | grep -F 'e2e user' | head -1)
check "project.json values arrive untouched" grep -qF '"tint":{"value":"0 0 1"}' <<<"$first"
check "--property overrides a value" grep -qF '"speed":{"value":7}' <<<"$first"
check "quotes, unicode and markup in a value survive" grep -qF '"motto":{"value":"say \"hi\" 右 </script>"}' <<<"$first"
check "applyGeneralProperties carries --fps" wait_log 'e2e general fps=30'
check "fetch of a local file" wait_log 'e2e fetch answer=42'
check "XMLHttpRequest of a local file" wait_log 'e2e xhr answer=42'
check "window.open is refused and alert does not block" wait_log 'e2e popup refused'
check "media plays without a click" wait_log 'e2e sound playing'
check "media integration is defined and silent" wait_log 'e2e api playing=1'
sleep 1; shot blue
check "the page is on screen (blue from project.json)" test "$(background blue)" = "0 0 255"

mark
send '{"cmd":"properties","values":{"tint":"1 0 0"}}'
check "a properties command reaches the listener with only what changed" wait_log 'e2e user {"tint":{"value":"1 0 0"}}'
sleep 0.5; shot red
check "and is visible (red)" test "$(background red)" = "255 0 0"

echo "frame rate"
fresh_second
check "--fps 30 holds requestAnimationFrame to 30 (got $(field frames))" between "$(field frames)" 27 33
mark; send '{"cmd":"fps","value":0}'
check "fps 0 lifts the cap and tells the page the output's rate" wait_log 'e2e general fps='
fresh_second
check "uncapped frames follow the display (got $(field frames))" between "$(field frames)" 50 400
mark; send '{"cmd":"fps","value":20}'
check "fps 20 is announced" wait_log 'e2e general fps=20'
fresh_second
check "and held (got $(field frames))" between "$(field frames)" 18 22

echo "sound"
check "Web Audio runs without a click ($(field webaudio))" test "$(field webaudio)" = running
check "--volume 50 halves what the page asked for (0.5 -> $(field volume))" test "$(field volume)" = "0.5/0.250"
send '{"cmd":"volume","value":100}'; fresh_second
check "volume 100 restores it ($(field volume))" test "$(field volume)" = "0.5/0.500"
send '{"cmd":"volume","value":0}'; fresh_second
check "volume 0 silences it ($(field volume))" test "$(field volume)" = "0.5/0.000"
send '{"cmd":"mute","value":true}'; send '{"cmd":"mute","value":false}'; send '{"cmd":"volume","value":100}'

echo "audio spectrum"
check "audio wanted event when the page registers a listener" wait_out '{"event":"audio","wanted":true}'
left=$(printf '0.25%.0s,' {1..64}); right=$(printf '0.75%.0s,' {1..64})
mark
for _ in 1 2 3; do send "{\"cmd\":\"audio\",\"left\":[${left%,}],\"right\":[${right%,}]}"; sleep 0.05; done
check "an audio frame arrives as 128 numbers, left then right" wait_log 'e2e audio length=128 left0=0.25 right0=0.75'

echo "pause and resume"
mark; send '{"cmd":"pause"}'
check "setPaused(true)" wait_log 'e2e paused true'
check "playing media is paused" wait_log 'e2e sound paused'
check "Web Audio is suspended" wait_log 'e2e webaudio suspended'
sleep 0.5; mark; shot paused1; sleep 2.5; shot paused2
check "timers do not run while paused" test "$(since | grep -cF 'e2e second')" = 0
check "nothing moves on screen while paused" test "$(magick compare -metric AE "$WORK/paused1.png" "$WORK/paused2.png" null: 2>&1 | cut -d' ' -f1)" = 0
send '{"cmd":"properties","values":{"tint":"0 1 0"}}'
check "a property changed during a pause still reaches the page" wait_log 'e2e user {"tint":{"value":"0 1 0"}}'
mark; send '{"cmd":"resume"}'
check "setPaused(false)" wait_log 'e2e paused false'
fresh_second
check "timers and frames run again (frames=$(field frames))" between "$(field frames)" 18 22
check "media that was playing plays again" test "$(field sound)" = playing
check "and so does Web Audio" test "$(field webaudio)" = running
shot resumed1; sleep 0.3; shot resumed2
check "the screen moves again" test "$(magick compare -metric AE "$WORK/resumed1.png" "$WORK/resumed2.png" null: 2>&1 | cut -d' ' -f1)" != 0

echo "staying put"
mark; send '{"cmd":"properties","values":{"leave":true}}'
check "navigation away is refused" wait_log 'tried to leave for https://example.com/'
fresh_second
check "and the wallpaper goes on" test -n "$(field frames)"
send '{"cmd":"properties","values":{"leave":false}}'
mark; send 'this is not json'; send '{"cmd":"dance"}'
check "a line that is not a command is logged and skipped" wait_log 'unknown command "dance"'

echo "pointer"
if [[ $have_pointer == 1 ]]; then
    mark; wlrctl pointer move 60 40; wlrctl pointer move -30 -20
    check "the page gets pointer input" wait_log 'e2e pointer moved' 4
else
    skip "pointer input (no wlrctl)"
fi

echo "a web process that dies"
# All of them: after a refused navigation WebKit may keep a spare one around.
web=$(for p in $(descendants "$PID"); do [[ $(cat "/proc/$p/comm" 2>/dev/null) == WebKitWebProces ]] && echo "$p"; done)
if [[ -n $web ]]; then
    # shellcheck disable=SC2086
    mark; kill -9 $web
    check "an error event" wait_out '"event":"error","message":"the web process ended'
    check "the page is loaded again with the properties as they were" wait_log '"tint":{"value":"0 1 0"}' 15
    wait_log 'e2e api playing=1' 5
    check "audio wanted is withdrawn and asked for again" test "$(grep '"event":"audio"' "$WORK/out" | tr -d '\n')" = \
        '{"event":"audio","wanted":true}{"event":"audio","wanted":false}{"event":"audio","wanted":true}'
    check "ready is said once" test "$(grep -c '"event":"ready"' "$WORK/out")" = 1
else
    fail "no web process found to kill"
fi

echo "leaving"
send '{"cmd":"quit"}'; wait_exit 5
check "quit: exit 0" test "$CODE" = 0
exec 3>&-

start --dir "$PAGE" --output "$OUTPUT" --passthrough --muted
check "ready again" wait_out '{"event":"ready"}' 20
if [[ $have_pointer == 1 ]]; then
    MARK=0; wait_log 'e2e second' 6
    mark; wlrctl pointer move 60 40; wlrctl pointer move -30 -20; sleep 1.5
    check "--passthrough: the page gets no pointer input" test "$(since | grep -cF 'e2e pointer moved')" = 0
else
    skip "--passthrough (no wlrctl)"
fi
exec 3>&-; wait_exit 5
check "end of stdin: exit 0" test "$CODE" = 0
PID=""

echo "a listener in a global variable, in a page with another name"
start --dir "$PAGE" --file global.html --output "$OUTPUT" --layer bottom
MARK=0
check "ready" wait_out '{"event":"ready"}' 20
check "var wallpaperPropertyListener is the window's listener" wait_log 'e2e global same=true'
check "and gets the properties" wait_log 'e2e global user tint,speed,motto,leave'
check "and the output's frame rate when there is no cap" wait_log 'e2e global general fps='
if [[ $have_hyprctl == 1 ]]; then
    check "--layer bottom" test "$(hyprctl layers -j | jq -r --arg o "$OUTPUT" '.[$o].levels["1"][].namespace')" = wepaper
fi
send '{"cmd":"pause"}'; send '{"cmd":"resume"}'
check "pause and resume in one breath arrive in order" wait_log 'e2e global paused false'
send '{"cmd":"quit"}'; wait_exit 5
check "quit: exit 0" test "$CODE" = 0
exec 3>&-
PID=""

echo
if [[ $FAILS == 0 ]]; then echo "all passed"; else echo "$FAILS failed"; fi
exit $((FAILS > 0))
