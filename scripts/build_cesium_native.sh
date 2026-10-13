#!/usr/bin/env bash
# Run from the repository root, inside nix develop.
set -euo pipefail

root="${CESIUM_BUILD_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/bevy-cesium/cesium-0.65.0}"
source_dir="$root/source"
install_dir="${CESIUM_NATIVE_DIR:-$root/install}"
native_commit=1bca8021711294cc83a5a032599ac72d9b3166c9
vcpkg_commit=56bb2411609227288b70117ead2c47585ba07713
export EZVCPKG_BASEDIR="$root/vcpkg"
mkdir -p "$root"
# Parallel CI jobs may share the same SDK cache. Serialize the native build.
if [[ "${CESIUM_BUILD_LOCKED:-}" != "$root" ]]; then
  exec uv run --no-project python - "$0" "$root" <<'LOCK'
import fcntl
import os
import subprocess
import sys
with open(os.path.join(sys.argv[2], ".build.lock"), "w") as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    env = dict(os.environ, CESIUM_BUILD_LOCKED=sys.argv[2])
    sys.exit(subprocess.run(["bash", sys.argv[1]], env=env).returncode)
LOCK
fi
if [[ -f "$install_dir/.cesium-native-revision" &&
      "$(cat "$install_dir/.cesium-native-revision")" == "$native_commit" &&
      -f "$install_dir/lib/libCesium3DTilesSelection.a" &&
      -f "$install_dir/include/Cesium3DTilesSelection/Tileset.h" ]]; then
    printf 'Using Cesium Native SDK: %s\n' "$install_dir"
    exit 0
fi
# NixOS cannot execute vcpkg's generic Linux tool binaries. Use shell tools.
export VCPKG_FORCE_SYSTEM_BINARIES=1
export VCPKG_MAX_CONCURRENCY="${VCPKG_MAX_CONCURRENCY:-${CMAKE_BUILD_PARALLEL_LEVEL:-4}}"
if [[ ! -d "$source_dir/.git" ]]; then
    git clone --depth 1 --branch v0.65.0 --recurse-submodules --shallow-submodules \
        https://github.com/CesiumGS/cesium-native.git "$source_dir"
fi
[[ "$(git -C "$source_dir" rev-parse HEAD)" == "$native_commit" ]] || {
    echo "Unexpected Cesium Native revision" >&2; exit 1;
}

# Draco 1.5.7 mistakes upstream Clang on Darwin for an ELF linker.
# Use an overlay, leaving both upstream checkouts unchanged.
vcpkg_dir="$EZVCPKG_BASEDIR/$vcpkg_commit"
# The Nix vcpkg wrapper otherwise redirects these outside the SDK cache.
export NIX_VCPKG_DOWNLOADS_ROOT="$vcpkg_dir/downloads"
export NIX_VCPKG_BUILDTREES_ROOT="$vcpkg_dir/buildtrees"
export NIX_VCPKG_PACKAGES_ROOT="$vcpkg_dir/packages"
export NIX_VCPKG_INSTALL_ROOT="$vcpkg_dir/installed"
if [[ ! -d "$vcpkg_dir/.git" ]]; then
    git clone --filter=blob:none https://github.com/microsoft/vcpkg.git "$vcpkg_dir"
    git -C "$vcpkg_dir" checkout "$vcpkg_commit"
fi
mkdir -p "$root/overlay"
cp -R "$vcpkg_dir/ports/draco" "$root/overlay/"
uv run --no-project python - "$root/overlay/draco/portfile.cmake" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
text = path.read_text()
patch = '''file(READ "${SOURCE_PATH}/cmake/draco_targets.cmake" DRACO_TARGETS)
string(REPLACE "if(CMAKE_CXX_COMPILER_ID MATCHES \\"^Clang|^GNU\\")" "if(NOT APPLE AND CMAKE_CXX_COMPILER_ID MATCHES \\"^Clang|^GNU\\")" DRACO_TARGETS "${DRACO_TARGETS}")
file(WRITE "${SOURCE_PATH}/cmake/draco_targets.cmake" "${DRACO_TARGETS}")

'''
path.write_text(text.replace('vcpkg_cmake_configure(', patch + 'vcpkg_cmake_configure('))
PY
cmake_args=()
if command -v vcpkg >/dev/null 2>&1; then
    cmake_args+=("-DEZVCPKG_HOST_VCPKG=$(command -v vcpkg)")
fi
cmake -S "$source_dir" -B "$root/build" "${cmake_args[@]}" \
    -DCMAKE_BUILD_TYPE=Release -DCESIUM_TESTS_ENABLED=OFF \
    -DCESIUM_ENABLE_CLANG_TIDY=OFF -DCMAKE_INSTALL_PREFIX="$install_dir" \
    -DVCPKG_OVERLAY_PORTS="$root/overlay"
cmake --build "$root/build" --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-4}"
cmake --install "$root/build"
printf '%s\n' "$native_commit" > "$install_dir/.cesium-native-revision"
printf 'Build ready. Set CESIUM_NATIVE_DIR=%s\n' "$install_dir"
