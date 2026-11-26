param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$Executable
)

$ErrorActionPreference = 'Stop'
if (-not [Environment]::Is64BitProcess) { throw 'Run this test in 64-bit PowerShell.' }
$setup = (Get-Item -LiteralPath $Installer).FullName
$expectedHash = (Get-FileHash -LiteralPath $Executable).Hash
$manifest = Join-Path $PSScriptRoot '../Cargo.toml'
$metadata = cargo metadata --format-version 1 --no-deps --locked --manifest-path $manifest | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read the expected package version.' }
$version = ($metadata.packages | Where-Object name -eq 'deskisle').version

$directory = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs/DeskIsle'
$app = Join-Path $directory 'deskisle.exe'
$uninstaller = Join-Path $directory 'uninstall.exe'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'DeskIsle.lnk'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\DeskIsle'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$startup = (Get-ItemProperty -LiteralPath $runKey -Name DeskIsle -ErrorAction SilentlyContinue).DeskIsle

foreach ($entry in $directory, $shortcut, $key) {
    if (Test-Path -LiteralPath $entry) { throw "Existing installation found at $entry. Use a clean test profile." }
}
if ($startup -eq ('"{0}"' -f $app)) { throw 'The install path is already registered for startup.' }

function Invoke-Silent([string]$File) {
    $process = Start-Process -FilePath $File -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$File exited with code $($process.ExitCode)." }
}

try {
    Invoke-Silent $setup
    if ((Get-FileHash -LiteralPath $app).Hash -ne $expectedHash) { throw 'Installed executable differs from the build.' }
    $shell = New-Object -ComObject WScript.Shell
    if (!(Test-Path -LiteralPath $shortcut) -or $shell.CreateShortcut($shortcut).TargetPath -ne $app) {
        throw 'Start menu shortcut does not launch the installed executable.'
    }
    $registration = Get-ItemProperty -LiteralPath $key
    if ($registration.DisplayVersion -ne $version -or $registration.UninstallString -ne ('"{0}"' -f $uninstaller)) {
        throw 'Windows uninstall command or version does not match the installation.'
    }

    # A locked payload must survive a failed upgrade and remain uninstallable.
    [IO.File]::WriteAllText($app, 'Previous payload')
    $locked = [IO.File]::Open($app, 'Open', 'Read', 'Read')
    try {
        $blocked = Start-Process -FilePath $setup -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
        if ($blocked.ExitCode -eq 0 -or [IO.File]::ReadAllText($app) -ne 'Previous payload') {
            throw 'Setup reported success or damaged the locked executable.'
        }
    } finally {
        $locked.Dispose()
    }
    Invoke-Silent $setup
    if ((Get-FileHash -LiteralPath $app).Hash -ne $expectedHash) { throw 'Setup did not replace the old executable.' }
} finally {
    if (Test-Path -LiteralPath $uninstaller) { Invoke-Silent $uninstaller }
}

foreach ($entry in $directory, $shortcut, $key) {
    if (Test-Path -LiteralPath $entry) { throw "Uninstall left $entry behind." }
}
$after = (Get-ItemProperty -LiteralPath $runKey -Name DeskIsle -ErrorAction SilentlyContinue).DeskIsle
if ($startup -ne $after) { throw 'Installation changed an unrelated startup setting.' }
Write-Host 'Verified first install, replacement install, and uninstall.'
