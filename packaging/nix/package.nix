{
  lib,
  rustPlatform,
  makeWrapper,
  pkg-config,
  wrapGAppsHook4,
  wayland,
  gtk4,
  gtk4-layer-shell,
  webkitgtk_6_0,
  glib-networking,
  libglvnd,
  mpv-unwrapped,
  ffmpeg,
  pulseaudio,
  fontconfig,
}:

rustPlatform.buildRustPackage {
  pname = "wepaper";
  version = (lib.importTOML ../../Cargo.toml).workspace.package.version;

  src = lib.cleanSourceWith {
    src = ../..;
    filter = path: type: !(type == "directory" && builtins.elem (baseNameOf path) [ "target" ".git" ]);
  };

  cargoLock.lockFile = ../../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
    wrapGAppsHook4
  ];
  # The last four are the web wallpaper plugin's: a browser engine in a layer of its own.
  buildInputs = [
    wayland
    gtk4
    gtk4-layer-shell
    webkitgtk_6_0
    glib-networking
  ];

  # The plugin is a program of its own and not a default member of the workspace:
  # without it web wallpapers have nothing to draw them.
  cargoBuildFlags = [
    "-p"
    "wepaper"
    "-p"
    "wepaper-web"
  ];

  # Wrapped by hand below: the daemon needs none of what a GTK program does.
  dontWrapGApps = true;

  # The tests that matter need a GPU and a Wallpaper Engine install; the rest are
  # run by CI.
  doCheck = false;

  postInstall = ''
    mkdir -p $out/share/wepaper
    cp -r plugins $out/share/wepaper/plugins
    install -Dm644 packaging/systemd/wepaper.service $out/lib/systemd/user/wepaper.service
    substituteInPlace $out/lib/systemd/user/wepaper.service --replace-fail '%h/.local/bin/wepaper' "$out/bin/wepaper"
  '';

  # EGL and libmpv are opened at run time, not linked; ffmpeg, parec and fc-match
  # are run when a wallpaper needs them.
  postFixup = ''
    wrapProgram $out/bin/wepaper \
      --prefix LD_LIBRARY_PATH : ${
        lib.makeLibraryPath [
          wayland
          libglvnd
          mpv-unwrapped
        ]
      } \
      --prefix PATH : ${
        lib.makeBinPath [
          ffmpeg
          pulseaudio
          fontconfig
        ]
      }
    wrapProgram $out/bin/wepaper-web "''${gappsWrapperArgs[@]}"
  '';

  meta = {
    description = "Wallpaper Engine wallpapers as the Wayland desktop background";
    homepage = "https://github.com/MoonTiwzy/wepaper";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "wepaper";
  };
}
