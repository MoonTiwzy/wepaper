# The control protocol

The daemon listens on a Unix socket, `$XDG_RUNTIME_DIR/wepaper.sock` unless `--socket` says
otherwise. A client writes one JSON object per line and reads one JSON object per line, in the
same order. Any number of clients can be connected.

```text
> {"cmd":"get","path":"config/fps"}
< {"ok":true,"value":60}
> {"cmd":"set","path":"config/fps","value":30}
< {"ok":true,"value":null}
> {"cmd":"set","path":"config/colour","value":1}
< {"ok":false,"error":"there is no setting \"colour\" (settings: fps, volume, mute, ...)"}
```

A line that is not JSON, or not a request, is answered with an error and the connection stays
open. The command line (`wepaper get`, `wepaper set`, ...) is a thin client of this protocol,
and the [`wepaper-ipc`](../ipc) crate is the same client as a Rust library.

## The tree

Everything the daemon knows is a node of one tree. Paths are written with `/`. An output's
name may be `*`, which means every output.

| path | what | set |
|---|---|---|
| `config/<key>` | a setting; the keys are those of the [config file](configuration.md) | yes |
| `outputs/<name>/size` | the surface's size in device pixels, `[width, height]` | |
| `outputs/<name>/scale` | the output's scale | |
| `outputs/<name>/paused` | whether the output is frozen on its current frame | yes |
| `outputs/<name>/fill` | this output's own fill mode, `null` for the setting | yes |
| `outputs/<name>/look/rate` | how fast the wallpaper plays: 1 as made, 0 stands still | yes |
| `outputs/<name>/look/volume` | the wallpaper's share of the volume, 0 to 1 (a scene's and a page's sounds) | yes |
| `outputs/<name>/look/flip` | the picture mirrored | yes |
| `outputs/<name>/look/zoom`, `shift_x`, `shift_y` | which part of the wallpaper the output shows, on top of its fill: enlarged about the output's middle (1 as the fill has it, below 1 smaller, 0.05 to 20), moved right and up by widths and heights of the output (-20 to 20). The wallpaper itself is framed: what the fill cropped comes into view | yes |
| `outputs/<name>/look/brightness`, `contrast`, `saturation`, `hue` | its colours: 1 is as it is, the hue turns in degrees; `null` puts any of a look's values back | yes |
| `outputs/<name>/fps` | frames drawn in the last full second | |
| `outputs/<name>/error` | why the last wallpaper asked for did not show, or `null` | |
| `outputs/<name>/wallpaper` | `null`, or the playing wallpaper: | |
| `.../wallpaper/path`, `kind`, `title` | what it is | |
| `.../wallpaper/properties/<name>` | a user property's value | yes |
| `.../wallpaper/schema` | the user properties' descriptions: label, type, range, options, default, condition | |
| `.../wallpaper/scene/zoom`, `clear` | the camera's zoom and the colour behind everything | yes |
| `.../wallpaper/scene/bloom` | whether the scene blooms (only a scene made with bloom can) | yes |
| `.../wallpaper/scene/parallax`, `parallax_amount`, `parallax_delay`, `parallax_mouse` | the camera following the pointer | yes |
| `.../wallpaper/scene/shake`, `shake_amplitude`, `shake_roughness`, `shake_speed` | the camera shaking | yes |
| `.../wallpaper/scene/ambient`, `skylight` | the light lit surfaces get from above and from below | yes |
| `.../wallpaper/layers/<id>/visible`, `origin`, `scale`, `angles`, `parallax_depth` | a layer's place in the scene | yes |
| `.../wallpaper/layers/<id>/alpha`, `color`, `brightness` | an image or text layer's look | yes |
| `.../wallpaper/layers/<id>/text` | a text layer's text | yes |
| `.../wallpaper/layers/<id>/volume` | a sound layer's own volume, 0 to 1 | yes |
| `.../wallpaper/layers/<id>/constants/<name>` | a constant of the layer's own shader | yes |
| `.../wallpaper/layers/<id>/effects/<n>/visible` | whether an effect runs | yes |
| `.../wallpaper/layers/<id>/effects/<n>/constants/<name>` | a constant of the effect's shaders: its strength, speed, colours, ... | yes |
| `.../wallpaper/skipped` | what of the wallpaper could not be loaded, one line each | |
| `outputs/<name>/device` | whether the output is a [device](devices.md), not a screen | |
| `devices/<name>` | a device: `size`, `fps`, `exec`, `enabled`; `null` takes it away | yes |
| `devices/<name>/size`, `fps`, `exec`, `enabled` | one field of it | yes |
| `devices/<name>/running`, `sent`, `error` | whether its program is up, the frames it was handed, why it is not running | |
| `plugins/<id>/enabled` | whether a plugin is on | yes |
| `plugins/<id>/params/<name>` | a shader plugin's parameter | yes |
| `memory/textures`, `buffers`, `video` | bytes of video memory, by what they hold | |
| `memory/videos` | how many videos are being decoded | |
| `memory/optimising` | how many videos are being re-encoded into cheaper copies | |
| `memory/listening` | whether the system's sound is being recorded for the spectrum | |
| `memory/heap/used`, `free`, `blocks` | bytes the allocator has in use, holds free for the next request, and took from the system as blocks of their own | |

Vectors (`origin`, `color`, ...) are arrays of numbers; on `set` an array, a string of numbers
(`"1 0.5 0"`) or one number for all components is taken.

### What is remembered

What you `set` under `wallpaper/` belongs to that wallpaper, not to the moment: it is kept in
`$XDG_STATE_HOME/wepaper/wallpapers/<id>.json` (next to the state file, wherever `--state`
puts it) and comes back whenever the wallpaper is shown again, on any output, after any
restart. `reset` forgets: `outputs/<name>/wallpaper` everything, `.../wallpaper/properties`
the user properties, `.../wallpaper/layers/12` or `.../wallpaper/scene/zoom` what was changed
under that path. A `show` that names `properties` starts the wallpaper with exactly those
values over its own; a `show` without them starts it as it was left.

## Requests

| `cmd` | fields | answer |
|---|---|---|
| `version` | | `{version, protocol, features, gpu}` |
| `get` | `path` (empty or absent: the whole tree) | the value, or the whole branch |
| `status` | | the same as `get` without a path |
| `set` | `path`, `value` | `null` |
| `list` | `path` | the names under the path |
| `reset` | `path` | `null`. `config` or `config/<key>`: back to the config file; `outputs/<name>/wallpaper` and anything under it: back to the wallpaper's own values |
| `show` | `wallpaper` (a folder or a workshop id), `output`?, `properties`? (an object; absent: the values it was left with) | `null` once it shows; an error, and the old wallpaper stays, when it cannot |
| `hide` | `output`? | `null` |
| `pause`, `resume` | `output`? | `null` |
| `snapshot` | `file`, `output`? | `null`; what the output shows right now is written as a PNG |
| `watch` | | `null`, and from then on the connection carries events |
| `reload` | | `null`; the config file is read again and what was set over the socket is forgotten |
| `quit` | | `null`; the daemon stops (`wepaper quit` waits until it is gone, and stops one that does not answer as a process) |

Without `output`, or with `"*"`, a request applies to every output.

## Events

After `watch`, the connection carries one event per line and requests on it are ignored:

```text
< {"event":"changed","what":"shown","value":"/.../431960/3566329932"}
< {"event":"changed","what":"paused","value":true}
< {"event":"changed","what":"config/fps","value":30}
```

A watcher never slows the daemon down: one that leaves more than a megabyte unread is
disconnected.

## Versions

`version` answers `{"protocol": 1, ...}`. The protocol only grows: fields and commands are
added, never changed or removed, so a client written for an older daemon keeps working.
