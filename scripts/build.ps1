# Builds Audian (daemon + helper processes).
#
#   .\scripts\build.ps1            # release build
#   .\scripts\build.ps1 -Debug     # debug build
#   .\scripts\build.ps1 -Package   # release build + copy binaries into .\dist
#
# Requirements: Rust (MSVC toolchain), Visual Studio Build Tools with the C++ workload and the
# "C++ Clang Compiler for Windows" component (libclang is needed by bindgen), and CMake.

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

# Keep this PC's folders (and so the user name) out of the binaries. The absolute paths of
# dependency sources (under CARGO_HOME) and of build outputs (under this folder) would otherwise
# end up in Rust panic messages and in whisper.cpp / llama.cpp assertion strings.
# Changing these flags triggers a full rebuild.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$env:CARGO_ENCODED_RUSTFLAGS = @("--remap-path-prefix=$cargoHome=cargo", "--remap-path-prefix=$root=audian") -join [char]0x1f
if ($cargoHome -notmatch " ") {
    # cc-rs splits these on spaces, so only a path without spaces can go here.
    $env:CFLAGS = "/d1trimfile:$cargoHome\"
    $env:CXXFLAGS = $env:CFLAGS
}
# CMake passes these to the compiler verbatim, so quoted paths with spaces work. They replace
# the defaults in .cargo/config.toml, keeping its optimisation flags.
$trim = "/d1trimfile:`"$root`" /d1trimfile:`"$cargoHome`""
$env:CMAKE_C_FLAGS_RELEASE = "/O2 /Ob2 /DNDEBUG $trim"
$env:CMAKE_CXX_FLAGS_RELEASE = "/O2 /Ob2 /DNDEBUG $trim"

Push-Location $root
try {
    if ($Debug) { cargo build --workspace } else { cargo build --workspace --release }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    if ($Package) {
        $out = Join-Path $root "dist"
        New-Item -ItemType Directory -Force $out | Out-Null
        foreach ($exe in "audian.exe", "audian-stt.exe", "audian-llm.exe") {
            Copy-Item (Join-Path $root "target\release\$exe") $out -Force
        }
        Write-Host "Packaged binaries in $out"
    }
} finally {
    Pop-Location
}
