# Packages qplayer.exe + its FFmpeg DLL closure into a self-contained, shareable folder + zip.
# DLLs sit next to the exe so colleagues need no PATH setup. Re-run after a rebuild.
# ponytail: DLL list is the runtime closure observed via process module enumeration; if a
#   future feature pulls in a new DLL, add it here.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent | Split-Path -Parent   # repo root
$exe  = Join-Path $root 'examples\qplayer\target\release\qplayer.exe'
$obs  = 'C:\Program Files\obs-studio\bin\64bit'                 # FFmpeg 7.x shared build source
$out  = Join-Path $root 'dist\qplayer'

if (-not (Test-Path $exe)) { throw "Build first: cargo run --release --manifest-path examples/qplayer/Cargo.toml -p qplayer  (or build). Missing: $exe" }

$dlls = 'avcodec-61','avdevice-61','avfilter-10','avformat-61','avutil-59',
        'librist','libx264-164','srt','swresample-5','swscale-8','zlib' | ForEach-Object { "$_.dll" }

Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item $exe $out

foreach ($d in $dlls) {
    $src = Join-Path $obs $d
    if (-not (Test-Path $src)) { throw "Missing FFmpeg DLL: $src" }
    Copy-Item $src $out
}

# VC++ runtime (app-local, redistributable) so machines without the redist still run.
foreach ($d in 'vcruntime140.dll','vcruntime140_1.dll','msvcp140.dll') {
    $src = Join-Path $env:SystemRoot "System32\$d"
    if (Test-Path $src) { Copy-Item $src $out }
}

@"
qplayer (Windows)

Just run qplayer.exe. All required DLLs are in this folder.
If Windows blocks it ("Windows protected your PC"), click More info > Run anyway.
"@ | Out-File (Join-Path $out 'README.txt') -Encoding utf8

$zip = Join-Path $root 'dist\qplayer-windows.zip'
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$out\*" -DestinationPath $zip
"Packaged: $out"
"Zip:      $zip ($([math]::Round((Get-Item $zip).Length/1MB,1)) MB)"
