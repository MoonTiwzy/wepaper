# Devices

A device is an output that is not a screen: a size, a rate and a program. The daemon draws
the device's wallpaper at that size, off the screen, and writes each finished frame to the
program's standard input. What the program does with the frames is its business: the panel
on a cooler's pump, a keyboard's display, a strip of lights, a stream, a file.

In everything else a device is an output like a monitor. It is listed under `outputs/`, a
wallpaper is put on it with `wepaper show <wallpaper> --on <name>`, it has its own `fill`
(`cover`, `contain`, `stretch`) and its own `look` (`zoom`, `shift_x`, `shift_y`, `flip`,
`rate`, the colours), it can be paused, and what it showed is remembered.

## Describing one

In the config file:

```jsonc
{
  "devices": {
    "panel": { "size": [480, 480], "fps": 30, "exec": "send-frames" }
  }
}
```

or while the daemon runs:

```sh
wepaper set devices/panel '{"size": [480, 480], "fps": 30, "exec": "send-frames"}'
wepaper set devices/panel/fps 20          # one field
wepaper set devices/panel/enabled false   # kept, but no output for now
wepaper set devices/panel null            # gone
wepaper reset devices                    # back to the config file's
```

| field | default | |
|---|---|---|
| `size` | none, required | the picture's size in pixels: `[width, height]` or `"480x480"` |
| `exec` | none, required | the program, run through `sh -c` |
| `fps` | `30` | frames a second at most, 1 to 240 |
| `enabled` | `true` | off, the device is kept in the list and is not an output |

A device's name is its output's name: letters, digits, `-`, `_`, `.` and spaces, and not the
name of a monitor. What is set over the socket is kept in the state file and wins over the
config file until `wepaper reload` or `wepaper reset devices`, like the settings.

`devices/<name>` in the control tree also says `running` (the program is up), `sent` (frames
handed to it since it was started) and `error` (why it is not running when it should be).

## What the program gets

Raw frames on its standard input: `width * height * 3` bytes each, red, green, blue for
every pixel, rows from the top, nothing before, between or after them. Its environment has
`WEPAPER_DEVICE` (the name), `WEPAPER_WIDTH`, `WEPAPER_HEIGHT`, `WEPAPER_FPS` and
`WEPAPER_SOCKET`. In the examples here `send-frames` stands for whatever program takes the
frames for your device.

```sh
# what ffmpeg calls it
ffmpeg -f rawvideo -pix_fmt rgb24 -s "${WEPAPER_WIDTH}x${WEPAPER_HEIGHT}" -r "$WEPAPER_FPS" -i - out.mkv
```

- The program is started when the device has its first frame, and its input is closed when
  nothing is shown on the device any more (`hide`), when the device is turned off or taken
  away, when its size or program changes, and when the daemon stops. A program that reads
  to the end of its input ends by itself; one that has not after three seconds is killed.
- The daemon never waits for the program. One frame at most waits to be written; when the
  program is slower than the device's rate, the frames in between are dropped and it gets
  the newest. A slow program cannot hold the monitors' wallpapers up.
- A video wallpaper only makes a frame when the video has a new one, so a device showing a
  24 frames a second video gets 24 frames a second, whatever its `fps`.
- A program that exits is started again with the next frame: at once after a long run, after
  a growing pause (up to a minute) when it keeps dying. `devices/<name>/error` says so.
- Its standard error is the daemon's: what it prints is in the daemon's log.

## What is different from a monitor

- There is no pointer on a device: its wallpaper sees the pointer in the middle.
- A wallpaper a renderer plugin draws (a web wallpaper) is drawn for a device by the plugin:
  the daemon starts the device's program and hands its input to the plugin, which draws
  without a surface anybody sees and writes the frames itself ([plugins.md](plugins.md)).
  The page is laid out as large as a monitor would have it (the monitor's shape, at most
  Full HD), and the device gets the part of it the device's `fill`, `look/zoom` and shifts
  frame, as with a scene; the page is drawn only as finely as that part needs. A snapshot
  of such a device is cut from the page the plugin writes while snapshots are asked for.
- Under `contain`, the bars have the scene's own background colour, as on a monitor
  (`outputs/<name>/wallpaper/scene/clear` changes it).
- A wallpaper that is on a monitor too is heard there, not twice.
