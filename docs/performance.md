# Performance

What a playing wallpaper costs, measured, and how to measure it yourself.

## The numbers

<!-- footprint:start -->
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
