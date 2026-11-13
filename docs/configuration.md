# Configuration

## The config file

`$XDG_CONFIG_HOME/wepaper/config.json` (usually `~/.config/wepaper/config.json`), or the file
named with `wepaper daemon --config`. It is JSON in which comments and trailing commas are
allowed. There need not be one: every setting has a default.

| key | default | what it does |
|---|---|---|
| `fps` | `0` | The most frames a second. `0` follows the display. A video under a cap also gets a copy re-encoded to that rate, so frames that would never be shown are not decoded. |
| `mute` | `true` | No sound. Muted, sound tracks and sound layers are not decoded at all. |
| `volume` | `100` | Volume, 0 to 100. |
| `fill` | `"cover"` | How a wallpaper fills an output of another shape: `cover` crops, `contain` adds bars, `stretch` distorts. An output can have its own (`outputs/<name>/fill`). |
| `layer` | `"background"` | The layer-shell layer of the surfaces. `bottom` is for desktops whose own wallpaper would otherwise cover it. |
| `passthrough` | `false` | The pointer goes through the wallpaper to what is under it. Off, the wallpaper gets the pointer (parallax, scripts that follow the cursor). |
| `audio_response` | `true` | Wallpapers that react to sound hear what the system plays. The system's sound is recorded only while a visible wallpaper actually reads the spectrum. |
| `video_hardware` | `false` | Decode video on the GPU. It takes less CPU and a few hundred megabytes more video memory; the default is the other way round. |
| `video_max_width` | `0` | Video frames are kept no wider than this, and wider videos get a re-encoded copy. `0` means the widest output. |
| `texture_max_side` | `0` | Textures larger than this on a side are loaded from a smaller level. `0` is no limit. A way to trade sharpness for video memory on a small GPU. |

## What wins

1. `wepaper daemon --set key=value` (for that run)
2. what was set over the socket (`wepaper set config/...`), remembered in the state file
3. the config file
4. the defaults

`wepaper reload` reads the file again and forgets (2). `wepaper reset config/<key>` forgets it
for one key.

## Files

| file | what |
|---|---|
| `$XDG_CONFIG_HOME/wepaper/config.json` | the settings; yours, never written by wepaper |
| `$XDG_CONFIG_HOME/wepaper/plugins/` | your plugins |
| `$XDG_STATE_HOME/wepaper/state.json` | what each output shows, with its property values, and settings changed over the socket |
| `$XDG_CACHE_HOME/wepaper/programs/` | linked shader programs, so a warm start compiles nothing |
| `$XDG_CACHE_HOME/wepaper/video/` | videos unpacked from scenes, and cheaper copies of oversized ones |
| `$XDG_RUNTIME_DIR/wepaper.sock` | the control socket |

Everything under the cache can be deleted at any time; it is rebuilt when needed.

## Environment

There are no feature switches in the environment. Two variables exist:

- `WEPAPER_LOG`: `error`, `warn` (the default), `info`, `debug` or `trace`. `-v` on the command
  line is `info`, `-vv` is `debug`.
- `WEPAPER_TEST_ASSETS`, `WEPAPER_TEST_WORKSHOP`: for the test suite, see
  [CONTRIBUTING.md](../CONTRIBUTING.md).
