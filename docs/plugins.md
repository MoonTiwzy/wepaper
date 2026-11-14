# Plugins

A plugin is a folder with a `plugin.json`. The daemon looks for them, later places winning
over earlier ones, in:

1. `<prefix>/share/wepaper/plugins/` next to the installed binary
2. every `wepaper/plugins/` under `$XDG_DATA_DIRS` (`/usr/share`, `/usr/local/share`)
3. `$XDG_DATA_HOME/wepaper/plugins/`
4. `$XDG_CONFIG_HOME/wepaper/plugins/`
5. folders named with `wepaper daemon --plugins DIR`

A plugin's id is its folder's name unless the manifest gives one. `wepaper list plugins` shows
what was found; a manifest that cannot be read is reported in the daemon's log and skipped.

```jsonc
{
  "name": "Dim",                 // for people
  "kind": "shader",              // hook | service | shader | renderer
  "enabled": true                // default true; `wepaper set plugins/<id>/enabled false`
  // ... and the fields of its kind, below
}
```

## `hook`: a command run when something happens

```json
{ "kind": "hook", "events": ["shown"], "exec": "./on-shown.sh" }
```

`exec` is run through `sh -c` in the plugin's folder, once per event, and is not waited for.
`events` lists what it runs on; left out, it runs on everything.

| event | when |
|---|---|
| `started`, `stopping` | the daemon comes up, goes down |
| `shown` | a wallpaper is on an output |
| `hidden` | an output's wallpaper was taken off |
| `paused`, `resumed` | an output was frozen, plays again |

| variable | |
|---|---|
| `WEPAPER_EVENT` | the event |
| `WEPAPER_OUTPUT` | the output's name |
| `WEPAPER_WALLPAPER`, `WEPAPER_TITLE`, `WEPAPER_KIND` | the wallpaper: its folder, title and kind |
| `WEPAPER_SOCKET` | the control socket, for `wepaper --socket "$WEPAPER_SOCKET" ...` |

See [`plugins/example-hook`](../plugins/example-hook).

## `service`: a process the daemon keeps running

```json
{ "kind": "service", "exec": "./battery-saver.py" }
```

Started with the daemon and stopped with it, with `WEPAPER_SOCKET` in its environment. It is
an ordinary client of the [control protocol](ipc.md): it can `watch` for events and `set`
anything. If it exits it is started again, at once after a long run and after a growing pause
(up to a minute) when it keeps dying.

## `shader`: a fragment shader over every frame

```json
{ "kind": "shader", "shader": "dim.frag", "params": { "u_Amount": { "default": 0.35, "min": 0, "max": 1 } } }
```

The shader is GLSL 330 with this interface:

```glsl
#version 330 core
uniform sampler2D u_Frame;      // the frame so far
uniform vec2 u_Resolution;      // its size in pixels
uniform float u_Time;           // seconds the wallpaper has played
in vec2 v_TexCoord;             // 0..1, y up
out vec4 color;
```

Every other `uniform float` is a parameter: its default comes from `params`, and
`wepaper set plugins/<id>/params/<name> <value>` changes it while it runs. Enabled shader
plugins run in the order of their ids; when none is enabled, frames go straight to the screen
and the feature costs nothing.

See [`plugins/dim`](../plugins/dim) and [`plugins/vignette`](../plugins/vignette).

## `renderer`: a process that draws a kind of wallpaper

```json
{ "kind": "renderer", "handles": ["web"], "exec": "wepaper-web" }
```

When a wallpaper's kind is one the daemon does not draw and a renderer plugin `handles` it,
the daemon starts the plugin's program once per output and leaves the drawing to it. The
program makes its own layer surface. It is started as

```
<exec> --dir <wallpaper folder> --file <start file> --output <output name>
       --layer background|bottom --namespace wepaper --fps <N> --volume <0..100> [--muted]
       [--passthrough] [--property <name>=<json>]...
```

and is told what changes on its standard input, one JSON object per line:

```
{"cmd":"properties","values":{"<name>": <json>, ...}}
{"cmd":"pause"}  {"cmd":"resume"}
{"cmd":"fps","value":30}  {"cmd":"volume","value":40}  {"cmd":"mute","value":true}
{"cmd":"quit"}
```

It exits on `quit` or when its input closes. If it dies, the output is left empty and
`outputs/<name>/error` says so. The bundled [`web`](../plugins/web) plugin is the reference
implementation.
