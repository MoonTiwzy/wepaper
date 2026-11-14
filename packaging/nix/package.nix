{
  lib,
  rustPlatform,
  makeWrapper,
  pkg-config,
  wayland,
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
  ];
  buildInputs = [ wayland ];

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
  '';

  meta = {
    description = "Wallpaper Engine wallpapers as the Wayland desktop background";
    homepage = "https://github.com/MoonTiwzy/wepaper";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "wepaper";
  };
}
