# Builds Audian (daemon + helper processes).
#
#   .\scripts\build.ps1            # release build
#   .\scripts\build.ps1 -Debug     # debug build
#   .\scripts\build.ps1 -Package   # release build + copy binaries into .\dist
#
# Requirements: Rust (MSVC toolchain), Visual Studio Build Tools with the C++ workload and the
# "C++ Clang Compiler for Windows" component (libclang is needed by bindgen), CMake, and the
# Vulkan SDK (https://vulkan.lunarg.com) for the GPU build of the rewriting engine.

param([switch]$Debug, [switch]$Package)
$ErrorActionPreference = "Stop"

# Make freshly installed tools visible even if this shell predates their installation.
$env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [Environment]::GetEnvironmentVariable("Path", "User")

if (-not $env:LIBCLANG_PATH) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vs = & $vswhere -products * -latest -property installationPath
        $clang = Join-Path $vs "VC\Tools\Llvm\x64\bin"
        if (Test-Path (Join-Path $clang "libclang.dll")) { $env:LIBCLANG_PATH = $clang }
    }
}
if (-not $env:LIBCLANG_PATH) { Write-Warning "libclang not found; bindgen may fail. Install the VS 'C++ Clang Compiler for Windows' component." }

$root = Split-Path -Parent $PSScriptRoot
# The Vulkan engine is built in a short folder: its shader-generator sub-build nests deep enough
# that, under this project's target folder, file paths pass the 260-character limit the C++
# compiler still has (error C1041).
$gpuTarget = Join-Path $env:LOCALAPPDATA "agb"

# Keep this PC's folders (and so the user name) out of the binaries. The absolute paths of
# dependency sources (under CARGO_HOME) and of build outputs (under this folder) would otherwise
# end up in Rust panic messages and in whisper.cpp / llama.cpp assertion strings.
# Changing these flags triggers a full rebuild.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$env:CARGO_ENCODED_RUSTFLAGS = @("--remap-path-prefix=$cargoHome=cargo", "--remap-path-prefix=$root=audian", "--remap-path-prefix=$gpuTarget=target") -join [char]0x1f
# /FS: llama.cpp's Vulkan build turns off MSBuild file tracking, after which parallel compiles
# (including its shader-generator sub-build, which only sees these variables) need /FS to share
# a .pdb file.
$env:CFLAGS = "/FS"
if ($cargoHome -notmatch " ") {
    # cc-rs splits these on spaces, so only a path without spaces can go here.
    $env:CFLAGS += " /d1trimfile:$cargoHome\"
}
$env:CXXFLAGS = $env:CFLAGS
# CMake passes these to the compiler verbatim, so quoted paths with spaces work. They replace
# the defaults in .cargo/config.toml, keeping its optimisation flags.
$trim = "/d1trimfile:`"$root`" /d1trimfile:`"$cargoHome`" /d1trimfile:`"$gpuTarget`""
$env:CMAKE_C_FLAGS_RELEASE = "/O2 /Ob2 /DNDEBUG $trim"
$env:CMAKE_CXX_FLAGS_RELEASE = "/O2 /Ob2 /DNDEBUG $trim"

# The Vulkan SDK: VULKAN_SDK if set, else the newest one installed in the usual places.
if (-not $env:VULKAN_SDK) {
    $sdk = @("$env:LOCALAPPDATA\VulkanSDK", "C:\VulkanSDK") | Where-Object { Test-Path $_ } |
        ForEach-Object { Get-ChildItem $_ -Directory } | Sort-Object Name -Descending | Select-Object -First 1
    if ($sdk) { $env:VULKAN_SDK = $sdk.FullName }
}
if (-not $env:VULKAN_SDK) { throw "Vulkan SDK not found (install it from https://vulkan.lunarg.com or set VULKAN_SDK)" }

Push-Location $root
try {
    $profileArgs = @()
    if (-not $Debug) { $profileArgs += "--release" }
    # Default members first, then the Vulkan build of the rewriting engine on its own: building
    # them together would turn on Vulkan for the CPU engine as well (Cargo feature unification).
    cargo build @profileArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    cargo build @profileArgs -p audian-llm-gpu --target-dir $gpuTarget
    if ($LASTEXITCODE -ne 0) { throw "cargo build (GPU engine) failed" }
    $config = if ($Debug) { "debug" } else { "release" }
    Copy-Item (Join-Path $gpuTarget "$config\audian-llm-gpu.exe") (Join-Path $root "target\$config\audian-llm-gpu.exe") -Force

    if ($Package) {
        $out = Join-Path $root "dist"
        New-Item -ItemType Directory -Force $out | Out-Null
        foreach ($exe in "audian.exe", "audian-stt.exe", "audian-parakeet.exe", "audian-llm.exe", "audian-llm-gpu.exe") {
            Copy-Item (Join-Path $root "target\release\$exe") $out -Force
        }
        Write-Host "Packaged binaries in $out"
    }
} finally {
    Pop-Location
}
