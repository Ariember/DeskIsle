$ErrorActionPreference = 'Stop'
$manifest = Join-Path $PSScriptRoot '..\Cargo.toml'
$target = 'x86_64-pc-windows-msvc'
$compiler = Get-Command makensis.exe -ErrorAction SilentlyContinue
if ($compiler) {
    $makensis = $compiler.Source
} else {
    $makensis = Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'
    if (!(Test-Path -LiteralPath $makensis)) { throw 'Install NSIS 3 to build the Windows installer.' }
}

$metadata = cargo metadata --manifest-path $manifest --format-version 1 --no-deps --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
$version = $metadata.packages[0].version
$release = Join-Path $metadata.target_directory "$target\release"
cargo build --manifest-path $manifest --target $target --release --locked
if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }

& $makensis /V3 /WX /INPUTCHARSET UTF8 "/DVERSION=$version" "/DRELEASE_DIR=$release" (Join-Path $PSScriptRoot 'installer.nsi')
if ($LASTEXITCODE -ne 0) { throw 'Installer build failed.' }
