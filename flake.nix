{
  description = "wepaper: Wallpaper Engine wallpapers as the Wayland desktop background";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: rec {
        wepaper = pkgs.callPackage ./packaging/nix/package.nix { };
        default = wepaper;
      });
    };
}
