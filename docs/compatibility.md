# Compatibility

What wepaper plays of Wallpaper Engine, what it does not, and how each part was checked.
`wepaper check <folder>` gives the same answer for your own library, wallpaper by wallpaper.

## How it was checked

There is no specification of Wallpaper Engine's formats. Everything here was worked out from
the files Wallpaper Engine itself installs (its shaders, effects, materials, scripts) and
from a workshop library of 113 wallpapers: 56 scenes, 51 videos and 6 web pages.

- **The whole library loads.** A test parses every package, texture, puppet, particle system
  and script in it, and `wepaper check` loads every wallpaper and compiles every shader
  variant it uses.
- **Frames are compared.** `tools/golden.sh` renders every scene and compares it with a
  reference frame; the scenes that differ most are looked at by eye, next to the workshop
  preview where that settles what is right.
- **The daemon is run for real**, in a compositor nobody sees (`tools/bed.sh`): layer
  surfaces, two outputs, hot-plug, the control tree, plugins.

## Scenes

| Feature | State |
|---|---|
| Image layers: materials, textures (raw, DXT1/3/5, embedded PNG and JPEG), mipmaps | yes |
| Blending: translucent, additive, normal; colour blend modes (multiply, screen, ...) | yes |
| Effects: chains, named buffers, copies, layers that read other layers | yes |
| Compose layers and fullscreen layers | yes |
| Sprite sheets (animated textures) | yes |
| Video textures (a video as a layer's picture) | yes |
| Text layers: wallpaper and system fonts, alignment, padding, text that changes | yes |
| Camera: zoom, parallax with pointer delay, shake | yes |
| Bloom | yes |
| User properties, with conditions; changed in place | yes |
| Keyframe animations: curves with Bezier handles, loop, mirror, single, events | yes |
| Particle systems: sprites, sprite trails, ropes, rope trails, children, control points | yes |
| Puppet warp: meshes, bones, animation layers, parts cut from one picture | yes |
| SceneScript: `update`, `init`, user properties, timers, cursor events, `shared` | yes |
| Sound layers | yes |
| Audio response: spectrum in shaders and in scripts | yes |
| Presets | yes |
| Perspective camera, 3D model objects, lights | no: drawn flat, objects left out |
| HDR pipeline | no: such scenes play in the normal range |
| Media integration: the player's state, title, artist and album in scripts (MPRIS) | yes |
| Media integration: the cover picture and its colours, the position in the track | no |

What a scene has that is not shown is listed by `wepaper inspect` and under `skipped` in the
control tree.

### Known differences

- **Shaders that lean on HLSL.** Wallpaper Engine's shaders are GLSL written with HLSL
  habits. Most are repaired on the way to the driver. Those that rely on implicit conversions
  everywhere compile only in NVIDIA's lenient mode; on Mesa such a pass is left out and
  named by `wepaper check`. One shader in the library is refused by every compiler: it is
  broken as its author published it.
- **Particles** are simulated by wepaper's own code. They follow the documents (emitters,
  initializers, operators), but turbulence is another noise field: the look matches, the
  individual particles do not. Sprite orientation other than towards the screen, emitter
  audio response and rope texture options are not there.
- **Puppets.** Inverse kinematics, bone physics and blend shapes are not there; the bones
  animations move are. Layer blend-in and blend-out times are read and not applied.
- **SceneScript.** Scripts run on QuickJS with Wallpaper Engine's own classes. Of the 627
  scripts in the library, 3 stop with an error, all three where the wallpaper asks for a
  layer it does not have. Not there: creating layers (`thisScene.createLayer`), camera
  transforms, texture animation control, `engine.openUserShortcut`. `localStorage` lasts as
  long as the wallpaper plays.
- **Text** is measured and drawn by wepaper: sizes agree with Wallpaper Engine's to a pixel
  or two, glyph shapes are the font's.

## Videos

Whatever libmpv plays. A video larger than the widest monitor, or faster than the frame cap,
gets a copy in the cache that is cheaper to decode; playback moves to it without a restart.

## Web

Through the `web` plugin, on WebKitGTK: `wallpaperPropertyListener`, pause,
frame cap, volume. The plugin takes an audio spectrum for pages that listen for one, but
the daemon does not send it yet; media integration and file properties are stubs. See
[plugins/web](../plugins/web/README.md).

## Application wallpapers

Not played, by design.

## Systems

- A Wayland compositor with `wlr-layer-shell`. Developed on Hyprland.
- OpenGL 3.3. Developed and measured on NVIDIA (driver 615); the code paths for Mesa are
  written and unmeasured.
- Wallpaper Engine installed through Steam, for its `assets` folder.
