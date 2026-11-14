# Architecture

wepaper is one binary and four crates:

```
src/        the binary
  daemon/   event loop, Wayland outputs and surfaces, frame pacing, the control socket, plugins
  cli/      the command line: the daemon's client and the offline tools
engine/     wepaper-engine: a wallpaper folder in, frames out. No Wayland, no sockets.
ipc/        wepaper-ipc: the protocol's messages and a blocking client
json/       wepaper-json: JSON as wallpapers actually write it
web/        wepaper-web: the web wallpaper renderer plugin, a process of its own
```

The engine is one crate on purpose. Its parts (formats, shaders, GPU, scene) are tightly
coupled, and splitting them into crates would draw borders where there are none. What other
programs use is the binary and the protocol, and those are what is separate.

## The path of a wallpaper

The engine's modules follow the path a wallpaper takes from disk to screen:

```
read/   bytes on disk become data          archives, layered files, textures, LZ4, pictures, puppets
doc/    JSON documents become typed values project, scene, material, effect, model
glsl/   the shader dialect becomes GLSL    includes, combos, annotations, repairs
gpu/    OpenGL                             the context, cached state, textures, programs, the buffer pool
share   the cache                          textures, programs, shader sources, fonts, videos
scene/  a scene becomes layers and passes  building, bindings, the tree, drawing; text, particle
                                           and puppet layers; the scene as the host of its scripts
particles/  particle systems               documents, the simulation, the vertices that draw them
script/     SceneScript                    a QuickJS runtime, the prelude, the host it runs against
video/  libmpv                             streams shared per file, cheaper copies, the video wallpaper
audio/  sound in and out                   sound layers; the spectrum of what the system plays
post    output shaders                     fragment shaders run over the finished frame
```

**read.** A wallpaper is a folder, usually with a `scene.pkg` in it, on top of Wallpaper
Engine's stock assets. `Files` stacks those and answers lookups the way Wallpaper Engine
does: case-insensitively, the wallpaper's files first. An archive is never loaded: its table
is read, and each file is read from its place on disk when asked for. A texture container is
parsed without touching its pixels; a level's bytes are read (and unpacked) only when that
level is going to the GPU.

**doc.** Wallpaper Engine's documents are dynamically typed: a field may be a literal, a
string of numbers, or an object that ties it to a user property, a script or an animation.
`Dyn<T>` is that shape. Parsing is by hand over a JSON value, because the format is loose
(byte order marks, trailing commas, `-nan(ind)` where a number should be) and a strict parser
would throw a whole wallpaper away over one token.

**glsl.** The shaders are GLSL written to also compile as HLSL. Translation is textual:
includes are inlined, the settings in comments are read (`// [COMBO]`, the JSON after each
`uniform`), and a prelude of `#define`s maps the dialect onto GLSL 330. The driver compiles
the result, as it is. Only when it refuses is anything repaired, one line at a time where the
driver points: the statement there is parsed and typed against what the shader declares
(`glsl/typed`: its variables, functions, structs, macros, the branches of its `#if`s that
are compiled), and the conversions HLSL makes by itself are written in, on the same lines, so
that the driver's next message still points at the right one. What that does not cover is
repaired from the message itself (an `#endif` too many, `%` between floats). Where the driver
has a lenient mode for HLSL's implicit conversions, the shader gets a second chance in it. Linked programs are saved
with `glGetProgramBinary`, keyed by source and driver, so the next start compiles nothing.

**gpu.** One context for the whole process, made current against each output's surface in
turn. `Gl` wraps the functions and caches the bindings it made. Textures, programs, meshes and
render targets delete themselves when dropped. Video memory is counted as it is allocated, by
what it is for.

**scene.** Building a scene turns documents into layers, and each layer into passes: a
program, where each texture slot's picture comes from, where each uniform's value comes
from, how it blends. Drawing a frame walks the layers.

- A layer without effects is one draw into the frame.
- A layer with effects runs its chain in buffers borrowed from the pool, and its last pass
  draws into the frame with the layer's transform. The buffers go back to the pool before the
  next layer starts.
- Compose and fullscreen layers read a copy of the frame so far.

**bindings.** Whatever can change a value is a binding from a source to a slot: a user
property, a keyframe animation, a script. Setting a property writes into the running scene;
nothing is rebuilt and nothing reloads. Each frame the animations move first, then the scripts
run (timers, the cursor's events, every `update`), and what they return goes into the same
slots.

**particles** and **script** know nothing of scenes. A particle system is simulated on the
CPU in flat arrays and hands back vertices for Wallpaper Engine's own particle shaders; the
scene gives it a place, a pointer and its property values. The script runtime holds one
JavaScript context per scene that has scripts and talks to a `Host` trait; the scene is the
host. Both are tested without a GPU.

**Puppets.** A puppet layer's effects run on the flat picture, as they were painted for it;
the mesh and its bones bend what comes out of them on the way into the frame.

**tree.** The same slots, addressed by path, are the wallpaper's branch of the control tree:
what `wepaper get` reads and `wepaper set` writes.

## Where the footprint comes from

| decision | what it saves |
|---|---|
| OpenGL through EGL, shaders compiled by the driver | an empty context costs about 34 MB of video memory instead of about 250 MB; no shader toolchain in the binary or the build |
| one context, one resource cache | a wallpaper on N outputs holds one copy of its textures, programs and video frames |
| per-output instances hold state only | a second output costs its surface and a few kilobytes |
| scenes draw into the surface | no full-screen buffer per output, no copy per frame |
| pooled effect buffers, sized by what reaches the screen | video memory follows the deepest effect chain, not the number of layers or the authored resolution |
| compressed textures stay compressed; unreachable levels are not read | a quarter to an eighth of the memory of decoding them |
| libmpv draws into our texture; no GPU decoder session unless asked | no copies through main memory; about 100 MB less video memory on NVIDIA |
| background copies of oversized videos, playback moves over | decode cost follows the monitor, not the file |
| a video plays as a tape of copies of itself | a loop has no seek in it: no frame is held, repeated or lost where it wraps |
| lazy audio capture, no sound decode when muted, video frames only when new | nothing runs for a wallpaper that does not use it |
| waiting for heavy frames asleep | a driver that spins a core while the GPU works is not given the chance |
| archives read in place, loader memory returned to the system | resident memory follows what plays, not what was loaded |

## The daemon

One thread, one loop. It waits in `poll` on the compositor's connection, the control socket,
its clients, and an eventfd that libmpv pokes when a video has a new frame. Each time round
it handles what came in and draws the outputs that are due: a scene on its frame callback
(and no faster than the frame cap), a video when its stream has a new frame.

Event handlers only record what the compositor said; the loop acts on the records. Nothing is
drawn or loaded from inside a handler.

The control socket speaks the [protocol](ipc.md): line-delimited JSON, answered in order, with
slow clients dropped rather than waited for.

One loop means that a call which never returns takes everything with it: the wallpaper
stands, `quit` gets no answer, a signal sets a flag nobody reads. So nothing the loop tells
a player waits for the player (libmpv's plain calls return when the player's own thread has
got to them, and that thread can stand in the audio output), a player is shut down on a
thread of its own, and a second thread watches the loop from outside: a daemon told to stop
is gone in three seconds, and one whose round takes half a minute exits with status 70 for
whoever started it to start it again.

## Plugins

The daemon stays small by leaving things out. [Plugins](plugins.md) are how they come back:
hooks and services for behaviour, shaders for the picture, renderers for kinds of wallpaper
the core does not draw. The web wallpaper renderer is a plugin and a process of its own, so
the browser engine it needs is not part of the daemon, and a page that crashes takes nothing
else with it.

## Testing

- **Unit tests** for everything pure: JSON, LZ4, archives, textures, puppets, documents, the
  shader translator and its repairs, bindings, keyframe animations, particles, scripts, the
  protocol, the server, settings, plugins.
- **Headless GPU tests**: a context without a display, real programs and targets; and whole
  scenes put together from Wallpaper Engine's stock pieces, when `WEPAPER_TEST_ASSETS` is set.
- **Corpus tests** against a real library, when `WEPAPER_TEST_ASSETS` and
  `WEPAPER_TEST_WORKSHOP` are set: every archive, texture, scene, puppet, particle system and
  script.
- **Golden comparison** (`tools/golden.sh`): every scene rendered headless and compared with
  reference frames.
- **The bed** (`tools/bed.sh`): a compositor nobody sees, to run the daemon in.
- **Footprint** (`tools/footprint.sh`): video memory, memory, CPU and load time per wallpaper.
