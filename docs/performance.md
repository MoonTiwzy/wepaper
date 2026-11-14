# Performance

What a playing wallpaper costs, measured, and how to measure it yourself.

## The numbers

<!-- footprint:start -->
| wallpaper | video memory, MiB | memory, MB | CPU, % of a core | start, ms |
|---|---|---|---|---|
| a picture with one effect | 84 → **65** | 125 → **31** | 2.3 → **2.5** | 300 → **89** |
| a picture and three particle systems | 95 → **87** | 142 → **40** | 3.6 → **2.9** | 341 → **149** |
| four pictures and particles | 143 → **125** | 169 → **42** | 4.4 → **3.7** | 387 → **130** |
| puppets, nine effect chains | 574 → **510** | 409 → **47** | 6.2 → **4.2** | 842 → **427** |
| 80 layers, effects on half | 269 → **152** | 233 → **49** | 10.4 → **7.1** | 664 → **196** |
| 168 layers, a scripted console | 2838 → **112** | 2509 → **53** | 94.0 → **11.1** | 559 → **152** |
| 190 layers, three video textures, scripts | 128 → **86** | 289 → **147** | 33.0 → **12.6** | 475 → **215** |
| 4K scene, 65 layers, 600 scripts | 482 → **392** | 514 → **62** | 12.0 → **11.7** | 1245 → **600** |
| scene around a 4K video | 113 → **90** | 294 → **141** | 52.2 → **29.7** | 343 → **174** |
| scene with a 4K video, sound, particles | 124 → **98** | 381 → **151** | 42.6 → **18.1** | 433 → **174** |
| video, 720p 30 fps | 84 → **46** | 199 → **95** | 8.8 → **8.5** | 323 → **174** |
| video, 4K 60 fps | 127 → **74** | 274 → **141** | 61.4 → **28.5** | 344 → **152** |
| video, 4K 120 fps | 127 → **74** | 402 → **139** | 51.8 → **28.4** | 367 → **152** |

Previous renderer → **wepaper**, same machine, one 2560x1440 output, 60 fps cap. On a typical wallpaper (the median of the thirteen measured) that is 1.3 times less video memory, 3.5 times less memory, 1.5 times less CPU and a start 2.3 times faster; on the heaviest one, 25 times less video memory and 47 times less memory.
<!-- footprint:end -->

"Previous renderer" is what WE Linux ran before wepaper existed, started the way WE Linux
started it, with its own memory-saving switches on. Same machine, same compositor, same
wallpapers, one after the other.

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
| CPU | CPU time of the process over the ten seconds, in percent of one core |

Video scenes and video wallpapers decode on the CPU by default, at the monitor's size: that is
where their CPU goes, and it is what keeps their video memory small. `video_hardware` moves
the decoding to the GPU.

## More outputs

A second output showing the same wallpaper costs its surface (about 33 MiB of video memory
at 1440p) and a few kilobytes of state: the textures, the programs and the video decoder are
the ones the first output uses.

<!-- outputs:start -->
<!-- outputs:end -->

## Building

<!-- build:start -->
<!-- build:end -->

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
