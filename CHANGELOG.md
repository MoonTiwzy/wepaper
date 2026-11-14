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
