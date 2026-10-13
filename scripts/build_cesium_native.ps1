# Windows/MSVC counterpart of build_cesium_native.sh.
$ErrorActionPreference = "Stop"
function Invoke-Checked {
    param([string]$Program, [string[]]$Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
$Root = if ($env:CESIUM_BUILD_ROOT) { $env:CESIUM_BUILD_ROOT } else { Join-Path $env:USERPROFILE ".cache/bevy-cesium/cesium-0.65.0" }
$Install = if ($env:CESIUM_NATIVE_DIR) { $env:CESIUM_NATIVE_DIR } else { Join-Path $Root "install" }
$Source = Join-Path $Root "source"
$Build = Join-Path $Root "build"
$env:EZVCPKG_BASEDIR = Join-Path $Root "vcpkg"
New-Item -ItemType Directory -Force $Root | Out-Null
if (!(Test-Path (Join-Path $Source ".git"))) {
    Invoke-Checked -Program git -Arguments @("clone", "--depth", "1", "--branch", "v0.65.0", "--recurse-submodules", "--shallow-submodules", "https://github.com/CesiumGS/cesium-native.git", $Source)
}
$Revision = & git -C $Source rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $Revision -ne "1bca8021711294cc83a5a032599ac72d9b3166c9") {
    throw "Unexpected Cesium Native revision"
}
Invoke-Checked -Program cmake -Arguments @("-S", $Source, "-B", $Build, "-A", "x64", "-DCESIUM_TESTS_ENABLED=OFF", "-DCESIUM_ENABLE_CLANG_TIDY=OFF", "-DCMAKE_INSTALL_PREFIX=$Install")
$Jobs = if ($env:CMAKE_BUILD_PARALLEL_LEVEL) { $env:CMAKE_BUILD_PARALLEL_LEVEL } else { "2" }
Invoke-Checked -Program cmake -Arguments @("--build", $Build, "--config", "Release", "--parallel", $Jobs)
Invoke-Checked -Program cmake -Arguments @("--install", $Build, "--config", "Release")
Write-Host "Build ready. Set CESIUM_NATIVE_DIR=$Install"
