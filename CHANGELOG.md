# Changelog

## Unreleased

- 3D scenes. A scene with a perspective camera is seen through it, or through its camera
  object, which parents, timelines and scripts move; parallax and shake move it too. Its
  model objects are drawn with their materials and Wallpaper Engine's lit shaders: several
  meshes a model, skeletons with their animation layers, objects attached to a model's
  bones, point and directional lights, the scene's ambient and sky light, reflections. Such
  a scene has a depth buffer; no other scene does.
- Scenes with `hdr` are drawn in half floats from the first layer to the last, with the
  high dynamic range bloom, and brought into the screen's range at the end. A scene
  without `hdr` has no float buffer and no extra pass.
- Shaders that ask for Wallpaper Engine's lighting compile; so do workshop shaders whose
  two stages declare a varying with different types. Two wallpapers of the library show
  an effect where they showed a white square.
- A material's constant can follow a user property, as a layer's values can.
- An animation that follows another (the editor's linked timelines) starts, stops and
  seeks with it, and a timeline's events reach every script of its layer.
- Particles take their colour from a list when their system gives one.
- Videos loop without a seam. An MP4 or MOV file plays as a tape of itself: the end of one
  copy runs into the start of the next with no seek, where the picture used to stand still
  for a quarter of a second every time round. Other containers still seek at their end.
- A frame's size is no longer asked of the player while it waits for the frame to be drawn,
  which could stall the drawing for a fifth of a second.
- A wallpaper is heard once. Shown on several outputs, it sounds on one of them: a scene's
  sound layers and a web page's music used to play on each, a moment apart. A video a scene
  uses as a picture is silent, where its sound track used to play over the scene's own
  sounds, on one output as well. A web page that starts its music before it is told the
  volume now plays at that volume, not at full.
- The players load none of mpv's own scripts, which were a few threads and a few megabytes
  for every video and every sound.
- Sound layers play the way the scene says. A layer that starts silent waits for a script;
  scripts play, pause and stop layers and ask whether one plays; a layer with several files
  plays them in turn, a `random` one picks a file and waits between two plays; a script, a
  user property or a timeline on a layer's volume moves it. A layer has a player only while
  it is heard: a scene with sixty silent layers used to start sixty players at once.
- The daemon cannot be left hanging. Nothing it tells a player waits for the player any
  more, and a player is shut down on a thread of its own. A daemon told to stop is gone in
  three seconds whatever it was doing, and one whose loop stands for half a minute gives up
  with status 70, so that whoever started it can start it again. `wepaper quit` returns
  when the daemon is gone, and stops one that does not answer as a process.

## 0.1.0

The first version.

- Scene wallpapers: image layers, materials and blend modes, effect chains, compose and
  fullscreen layers, text, sprite sheets, video textures, sound layers, camera parallax and
  shake, bloom, keyframe animations, particle systems, puppet warp, SceneScript, the audio
  spectrum, user properties that change in place, presets.
- Video wallpapers with cover, contain and stretch, sound, and background copies of videos
  larger or faster than the monitor needs.
- One daemon for every output, with one control tree over a socket: `get`, `set`, `list`,
  `reset`, `watch`, `show`, `hide`, `pause`, `resume`, `snapshot`. Layers, effects, shader
  constants and the scene's camera can be changed while it plays, and a wallpaper remembers
  what was changed in it.
- Plugins: hooks, services, output shaders, renderers. Bundled: `web`, `dim`, `vignette`.
- Offline tools: `render`, `inspect`, `check`, `unpack`.
