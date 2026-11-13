# Contributing

## Building

```sh
cargo build                 # the daemon, the command line, the engine
cargo build -p wepaper-web  # the web plugin (GTK 4, WebKitGTK 6, gtk4-layer-shell)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Tests that need a real library run when these are set, and are skipped otherwise:

```sh
export WEPAPER_TEST_ASSETS=~/.local/share/Steam/steamapps/common/wallpaper_engine/assets
export WEPAPER_TEST_WORKSHOP=~/.local/share/Steam/steamapps/workshop/content/431960
```

GPU tests need an EGL device that gives a desktop OpenGL context without a display (any
recent Mesa or NVIDIA driver); they skip themselves when there is none.

## Looking at what you changed

```sh
wepaper render <wallpaper> -o out.png --time 2      # a frame, no compositor needed
wepaper check <folder of wallpapers>                # what does not load, and why
tools/golden.sh <workshop> <reference frames> <out> # every scene against reference frames
tools/bed.sh up && tools/bed.sh run wepaper daemon  # the daemon in a compositor nobody sees
tools/footprint.sh ...                              # what a wallpaper costs
```

`tools/bed.sh` needs a headless host compositor that a nested Hyprland can run in; see the
comment at its top. Do not test the daemon on the session you are working in: a layer surface
on the background layer covers your desktop.

`-v` logs at info level, `-vv` at debug. At debug level a shader the driver refused is kept in
`$TMPDIR/wepaper-shaders/` as it was sent.

## Conventions

- **A broken part of a wallpaper is skipped, never fatal.** A missing texture, a shader that
  does not compile, a script that throws: log it once, leave that part out, show the rest.
- **Nothing per output that can be shared.** If two outputs showing one wallpaper would each
  hold a copy of something, it belongs in the cache.
- **Nothing runs until something needs it.** Look at how audio capture starts for the pattern.
- **No feature switches in the environment.** Behaviour lives in the config and the tree.
- **Every public item is documented**, comments say why rather than what, and tests are named
  by the behaviour they pin.
- `unsafe` is for the GPU, libmpv, EGL/Wayland glue and libc, and every block says why it is
  sound.

## Commits

Imperative subject lines that say what changes for someone using or reading the program.
One thing per commit where that is practical.
