# Performance

What a playing wallpaper costs, measured, and how to measure it yourself.

## The numbers

| wallpaper | video memory, MiB | memory, MB | of it, its own | CPU, % of a core | start, ms |
|---|---|---|---|---|---|
| a picture with one effect | 65 | 76 | 26 | 2.5 | 87 |
| a picture and three particle systems | 87 | 85 | 35 | 2.9 | 150 |
| four pictures and particles | 125 | 85 | 34 | 3.5 | 131 |
| puppets, nine effect chains | 478 | 91 | 39 | 4.3 | 432 |
| 80 layers, effects on half | 152 | 94 | 42 | 6.8 | 195 |
| 168 layers, a scripted console | 112 | 99 | 47 | 11.6 | 174 |
| 190 layers, three video textures, scripts | 86 | 219 | 132 | 14.5 | 216 |
| 4K scene, 65 layers, 600 scripts | 392 | 107 | 55 | 11.6 | 624 |
| scene around a 4K video | 90 | 213 | 126 | 30.3 | 173 |
| scene with a 4K video, sound, particles | 98 | 221 | 134 | 18.9 | 173 |
| video, 720p 30 fps | 46 | 166 | 80 | 6.7 | 174 |
| video, 4K 60 fps | 74 | 212 | 125 | 31.9 | 174 |
| video, 4K 120 fps | 74 | 211 | 125 | 27.3 | 152 |

One 2560x1440 output, 60 frames a second, sound off.

## What was measured, and how

- **Machine**: Ryzen desktop, GeForce RTX 5070 (driver 615), Linux 7.2.
- **Where**: a compositor nobody sees (`tools/bed.sh`): a headless host with a Hyprland nested
  in it, one 2560x1440 output. Nothing else draws there, so the numbers are the wallpaper's.
- **Frame cap**: 60 frames a second. Sound off.
- **How**: `tools/footprint.sh` starts the renderer fresh for each wallpaper, waits until its
  log says the wallpaper plays, lets it settle for six seconds, then watches the process for
  ten.

| column | what it is |
|---|---|
| start | milliseconds from launching the process to the wallpaper playing |
| video memory | what `nvidia-smi` charges the process, in MiB. An empty OpenGL context with one 1440p surface is about 34 of them |
| memory | proportional set size (`Pss` in `/proc/<pid>/smaps`), in MB: memory shared with other processes is counted by share |
| its own | the part of it that is the process's alone and cannot be dropped (`Private_Dirty`): its heap, the driver's buffers, decoded frames |
| CPU | CPU time of the process over the ten seconds, in percent of one core |

The memory column moves with the rest of the system. What is in it besides the process's own
is code: the program and, mostly, the graphics driver's libraries, which the kernel keeps
once for every program that has them and charges by share. The table was measured with
nothing else holding the driver's libraries in memory, so the renderer pays for them in
full: the worst case. On a desktop where other programs have them loaded, the first row
read 31. The column "its own" does not move.

Video scenes and video wallpapers decode on the CPU by default, at the monitor's size: that is
where their CPU goes, and it is what keeps their video memory small. `video_hardware` moves
the decoding to the GPU.

## More outputs

A second output showing the same wallpaper costs its surface (about 33 MiB of video memory
at 1440p) and a few kilobytes of state: the textures, the programs and the video decoder are
the ones the first output uses.

| the scene with a 4K video | video memory, MiB | memory | video decoders | threads |
|---|---|---|---|---|
| on one 2560x1440 output | 98 | | 1 | 15 |
| on two | 130 | 12 MB more | 1 | 15 |

Unplugging the second output gives its 32 MiB back at once.

## 3D and high dynamic range scenes

A flat scene pays nothing for either: no depth buffer, no float buffer, no extra pass, no
light uniforms, and its shaders compile as they did. A 3D scene draws into a buffer of the
output's size with a depth buffer (eight bytes a pixel in all, about 16 MiB at 1440p), and
a scene that reflects copies what is behind its first reflecting layer once a frame, at
half the size. A scene with `hdr` keeps its frame and its layers' effect buffers in half
floats, twice the memory of bytes, and its bloom takes a chain of smaller buffers, a third
of the frame's size in all, for the length of the pass that makes it.

## Building

| on a 32-thread desktop, from nothing | |
|---|---|
| debug build | 9.6 s |
| release build | 31 s |
| release build after a change to the engine | 9 s |

The release binary is 3.8 MB.

The dependency tree is small on purpose: no shader cross-compiler, no GUI toolkit, no async
runtime, no serde. The largest single item is QuickJS (C, compiled once) behind the `script`
feature; `--no-default-features` builds without it.

## Measuring your own

```sh
tools/bed.sh up                       # needs a headless-capable compositor; see the script
tools/footprint.sh mine out.tsv ~/.local/share/Steam/steamapps/workshop/content/431960/<id> -- \
    sh -c 'exec target/release/wepaper -v daemon "$WALLPAPER" --socket /tmp/fp.sock --state /tmp/fp.json'
tools/bed.sh down
```

`wepaper get memory` on a running daemon gives the same video memory split by what it holds:
textures, effect buffers, video frames.
