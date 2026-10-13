# Build the pinned native SDK without network access in the build sandbox.
{
  lib,
  stdenv,
  fetchurl,
  cmake,
  ninja,
  pkg-config,
  git,
  vcpkg-tool,
  curl,
  zip,
  unzip,
  autoconf,
  automake,
  libtool,
  perl,
  nasm,
  python3,
  getopt,
}: let
  nativeRev = "1bca8021711294cc83a5a032599ac72d9b3166c9";
  vcpkgRev = "56bb2411609227288b70117ead2c47585ba07713";
  nativeSource = builtins.fetchGit {
    url = "https://github.com/CesiumGS/cesium-native.git";
    rev = nativeRev;
    submodules = true;
  };
  vcpkgSource = builtins.fetchGit {
    url = "https://github.com/microsoft/vcpkg.git";
    rev = vcpkgRev;
  };
  sources = builtins.fromJSON (builtins.readFile ./vcpkg-sources.json);
  downloads =
    map (source: {
      inherit (source) name;
      path = fetchurl {inherit (source) url hash;};
    })
    sources;
in
  stdenv.mkDerivation {
    pname = "cesium-native";
    version = "0.65.0";
    src = nativeSource;
    nativeBuildInputs = [
      cmake
      ninja
      pkg-config
      git
      vcpkg-tool
      curl
      zip
      unzip
      autoconf
      automake
      libtool
      perl
      nasm
      python3
      getopt
    ];
    enableParallelBuilding = true;
    VCPKG_FORCE_SYSTEM_BINARIES = "1";
    VCPKG_DISABLE_METRICS = "1";
    VCPKG_BINARY_SOURCES = "clear";
    preConfigure = ''
      export HOME="$TMPDIR/home"
      export VCPKG_MAX_CONCURRENCY="$NIX_BUILD_CORES"
      mkdir -p "$HOME" "$TMPDIR/vcpkg"
      export EZVCPKG_BASEDIR="$TMPDIR/vcpkg"
      cp -R ${vcpkgSource} "$EZVCPKG_BASEDIR/${vcpkgRev}"
      chmod -R u+w "$EZVCPKG_BASEDIR/${vcpkgRev}"
      export VCPKG_ROOT="$EZVCPKG_BASEDIR/${vcpkgRev}"
      export NIX_VCPKG_DOWNLOADS_ROOT="$VCPKG_ROOT/downloads"
      export NIX_VCPKG_BUILDTREES_ROOT="$VCPKG_ROOT/buildtrees"
      export NIX_VCPKG_PACKAGES_ROOT="$VCPKG_ROOT/packages"
      export NIX_VCPKG_INSTALL_ROOT="$VCPKG_ROOT/installed"
      mkdir -p "$VCPKG_ROOT/downloads"
      ${lib.concatMapStringsSep "
" (source: ''
          cp ${source.path} "$VCPKG_ROOT/downloads/${source.name}"
        '')
        downloads}
      patchShebangs "$VCPKG_ROOT/scripts"
      # The SDK is installed and linked in Release; avoid duplicate Debug builds.
      for triplet in "$VCPKG_ROOT"/triplets/*.cmake; do
        printf '\nset(VCPKG_BUILD_TYPE release)\n' >> "$triplet"
      done
      # Never let vcpkg acquire unpinned sources or tools during the build.
      substituteInPlace cmake/ezvcpkg/ezvcpkg.cmake \
        --replace-fail "install --triplet" "install --no-downloads --triplet"
      # Draco's upstream Clang test assumes ELF unless Darwin is excluded.
      substituteInPlace "$VCPKG_ROOT/ports/draco/portfile.cmake" \
        --replace-fail 'vcpkg_cmake_configure(' 'file(READ "''${SOURCE_PATH}/cmake/draco_targets.cmake" DRACO_TARGETS)
      string(REPLACE "if(CMAKE_CXX_COMPILER_ID MATCHES \"^Clang|^GNU\")" "if(NOT APPLE AND CMAKE_CXX_COMPILER_ID MATCHES \"^Clang|^GNU\")" DRACO_TARGETS "''${DRACO_TARGETS}")
      file(WRITE "''${SOURCE_PATH}/cmake/draco_targets.cmake" "''${DRACO_TARGETS}")
      vcpkg_cmake_configure('
    '';
    cmakeFlags = [
      "-DCESIUM_TESTS_ENABLED=OFF"
      "-DCESIUM_ENABLE_CLANG_TIDY=OFF"
      "-DEZVCPKG_HOST_VCPKG=${vcpkg-tool}/bin/vcpkg"
    ];
    postInstall = ''
      printf '%s\n' '${nativeRev}' > "$out/.cesium-native-revision"
    '';
    meta = {
      description = "Cesium Native SDK used by bevy-cesium";
      homepage = "https://github.com/CesiumGS/cesium-native";
      license = lib.licenses.asl20;
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
    };
  }
