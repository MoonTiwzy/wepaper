# Changelog

## Unreleased

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
- A hidden layer that another layer reads is drawn, into its picture only. Scenes build one
  picture out of layers nobody is to see as they are (a globe out of a map, its clouds and
  a mask): the layer that put them together showed a white sheet instead.
- A daemon that has shown many wallpapers gives their memory back. The allocator kept the
  blocks pictures were decoded in once it had been given them (some 140 MB after a dozen
  scenes): such blocks are now mappings of their own, gone when freed. What a wallpaper
  that went left behind is released when it goes and once more a moment later, and the
  sources of shaders and the fonts nobody writes with are no longer kept.
- Two wallpapers with a shader of the same name each get their own: the second one shown
  was drawn with the first one's.
- More of what the original's shader compiler lets through is repaired: a second index on
  one float of an array (`spectrum[a][b]`), a float as an array index, a remainder of
  floats in the words of another driver. A line with several mistakes is repaired one
  after another.
- On drivers that take shaders by the letter (Mesa: AMD, Intel) a third fewer effects are
  left out. What the original converts by itself is converted where the driver says it is
  missing: what is assigned to a variable of another kind, what a function is given that
  it does not take, a comparison that is counted with, a value one stage writes longer
  than the other reads.
- On Mesa nearly all of the rest plays too: of the 59 effects and layers the test library
  still left out, 6 are. A strict driver says that two values did not fit, not which kinds
  they were, so the kinds are now worked out from what the shader declares (its variables,
  functions, macros, and only the branches of its `#if`s that are compiled), and the
  conversions the original makes by itself are written in: where two vectors of different
  lengths meet, the longer is cut; `int` and `uint` are made one; a number or a vector
  chooses where a condition is wanted, a vector a component at a time; a comparison
  counted with is a number; what is assigned, returned or given to a function is made the
  kind it goes to. A statement written over several lines is read whole. Where a kind is
  not certain, nothing is changed. Audio visualizers that were a white square, blurs and
  colour effects that were missing are there.
- `wepaper get memory/heap` says what the allocator has in use and what it holds free.
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
