# Changelog

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
