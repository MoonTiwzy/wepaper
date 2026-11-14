# web: web wallpapers

A renderer plugin: `wepaper-web` shows a Wallpaper Engine *web* wallpaper (a folder with an HTML
page) as the background of one output, with GTK 4, WebKitGTK 6 and gtk4-layer-shell. The daemon
starts one process per output and talks to it over stdin and stdout. The browser engine lives in its
own process on purpose: the daemon stays small, and a page that crashes cannot take the desktop with it.

Build: `cargo build --release -p wepaper-web` (not a default member of the workspace). Needs
`webkitgtk-6.0`, `gtk4` and `gtk4-layer-shell`.

## Command line

```
wepaper-web --dir <wallpaper folder> [--file index.html] --output <output name, e.g. DP-1>
            [--layer background|bottom] [--namespace wepaper] [--fps N] [--volume 0..100] [--muted]
            [--property name=json]...        # initial user property values
            [--passthrough]                  # the page gets no pointer input
```

`--file` is the `file` of the wallpaper's `project.json`. `--fps 0` is no cap. A `--property` value
that is not JSON is taken as text.

For a device (an output that is not a screen, `docs/devices.md`) there is no surface to show the
page on:

```
wepaper-web --dir <wallpaper folder> [--file index.html] --frames <WIDTH>x<HEIGHT> [--frames-fd 3]
            [--output <output name>] [--fps N] [--volume 0..100] [--muted] [--property name=json]...
```

The page lives in a window as large as the device that nobody sees (fully transparent, on the
background layer of `--output`, or of any output without it). `--fps` times a second (30 without
it) WebKit is asked for a picture of the page, and the picture is written to the descriptor as raw
RGB: three bytes a pixel, rows from the top, `WIDTH * HEIGHT * 3` bytes a frame. No frames are
written while paused. When the descriptor's reader is gone the plugin exits.

## stdin: commands from the daemon

One JSON object per line.

```
{"cmd":"properties","values":{"name": <json>, ...}}    user properties changed
{"cmd":"pause"} / {"cmd":"resume"}
{"cmd":"fps","value":30}                                0 lifts the cap
{"cmd":"volume","value":40} / {"cmd":"mute","value":true}
{"cmd":"audio","left":[64 floats],"right":[64 floats]}  an audio spectrum frame, about 30 a second while a listener is registered
{"cmd":"quit"}
```

A line that does not decode is logged and skipped.

## stdout: events to the daemon

One JSON object per line.

```
{"event":"ready"}                       once: the page has loaded and the surface is mapped
{"event":"audio","wanted":true|false}   the page registered or dropped an audio listener
{"event":"error","message":"..."}       fatal when the process exits right after
```

Exit code 0 on `quit` or end of stdin (the daemon is gone), 2 when the output does not exist or goes
away, 1 on any other fatal error; an `error` event comes first. Log lines go to stderr as
`wepaper-web: <level>: <message>` (`error`, `warning`, `info`, and `debug` with `WEPAPER_WEB_DEBUG`
set). Script errors of the page are logged as warnings starting with `page:`.

## What the page gets

- A layer-shell surface on the named output only (matched by connector name), anchored to all four
  edges, exclusive zone -1, no keyboard input, black until the page has painted.
- Wallpaper Engine's JavaScript API, injected before the page's own scripts:
  - `window.wallpaperPropertyListener`: `applyUserProperties(properties)` with
    `{name: {value: ...}}`, once after load with every property (`project.json`'s
    `general.properties.<name>.value`, overridden by `--property`), then on each `properties`
    command with the changed ones; `applyGeneralProperties({fps})`; `setPaused(isPaused)`. The
    listener may be assigned at any time (before load, in `onload`, later, as a global `var`), and
    its methods may be attached after the assignment. `userDirectoryFilesAddedOrChanged` and
    `userDirectoryFilesRemoved` are never called.
  - `window.wallpaperRegisterAudioListener(callback)`: 128 floats (64 left, then 64 right) per
    `audio` command.
  - `wallpaperRegisterMediaStatusListener`, `...PropertiesListener`, `...ThumbnailListener`,
    `...PlaybackListener`, `...TimelineListener`, `wallpaperRequestRandomFileForProperty` and
    `window.wallpaperMediaIntegration`: defined, never fire.
- The page is opened as `file://` with access between local files allowed, as Wallpaper Engine
  opens it: relative URLs, `fetch`/`XMLHttpRequest` of the wallpaper's files, WebGL textures and
  media work. The wallpaper's folder is the only part of the disk the sandboxed web process is
  given. Requests to the network are allowed (wallpapers fetch weather and the like).
- The main frame cannot leave the wallpaper's folder; popups, dialogs (`alert` returns at once),
  the context menu, file choosers, permission requests, fullscreen and notifications are refused.
- Media plays without a click.

## How the commands are carried out

- `pause`: `setPaused(true)`, playing `<audio>`/`<video>` paused, audio contexts suspended, CSS
  animations paused, and the page's `requestAnimationFrame`, `setTimeout` and `setInterval`
  callbacks held, so the page asks for no frames and the last one stays up. WebKitGTK has no call
  that suspends a page, so this is done by the injected script: timers created by a string of
  code, transitions, animated images and web workers are not stopped. `resume` undoes all of it;
  timeouts that came due in between run then.
- `fps`: `applyGeneralProperties({fps})`, and `requestAnimationFrame` callbacks are held to that
  rate. Without a cap the page is told the output's refresh rate.
- `volume`: WebKitGTK can mute a view but has no volume for it, so the injected script scales media
  elements (the page still reads back the volume it set) and puts a gain in front of every audio
  context's output. `mute` is WebKit's own.
- A web process that dies is reported as an `error` event and the page is loaded again in its
  current state, three times at most.

## Known limits

- Nothing is stored: cookies, `localStorage` and caches live in memory and end with the process.
- Media integration, file and directory properties are stubs.
- The API, pause and the frame cap apply to the page itself, not to frames (`<iframe>`) inside it.
- One process tree per output (the plugin, a web process, a network process); they share
  nothing between outputs.
- With NVIDIA's driver the plugin sets `WEBKIT_GST_DISABLE_GL_SINK=1` unless the variable is
  already set: WebKit's OpenGL video sink paints `<video>` green there.

## Tests

- `cargo test -p wepaper-web`: arguments, protocol, properties, the scripts sent to the page, and
  (with node installed) the injected script in a scripted page, `web/tests/bridge.test.js`.
- `web/tests/e2e/run.sh <wepaper-web binary> [output]`: the plugin on a real compositor showing
  `web/tests/e2e/wallpaper/`. Run it in a nested or headless compositor only; it refuses the
  session's own runtime directory.
