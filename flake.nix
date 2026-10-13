{
  description = "Standalone Cesium Native + Bevy development";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = {
    self,
    nixpkgs,
  }: let
    systems = ["aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux"];
    eachSystem = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs {inherit system;}));
  in {
    devShells = eachSystem (pkgs: {
      default = pkgs.mkShell {
        packages = with pkgs; [rustc cargo rustfmt clippy clang cmake ninja git pkg-config uv just curl zip unzip autoconf automake libtool perl bison nasm];
        buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (with pkgs; [wayland libxkbcommon libX11 libXcursor libXi libXrandr vulkan-loader]);
        shellHook = ''
          export CARGO_BUILD_JOBS="''${CARGO_BUILD_JOBS:-2}"
          export CMAKE_BUILD_PARALLEL_LEVEL="''${CMAKE_BUILD_PARALLEL_LEVEL:-2}"
        '';
      };
    });
  };
}
