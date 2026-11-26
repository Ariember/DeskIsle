$ErrorActionPreference = 'Stop'
$payload = $env:DESKISLE_UPDATE_PACKAGE
$parentId = [int]$env:DESKISLE_UPDATE_PID
$env:DESKISLE_UPDATE_PACKAGE = $null
$env:DESKISLE_UPDATE_PID = $null
$target = [IO.Path]::Combine([Environment]::GetFolderPath('LocalApplicationData'), 'Programs\DeskIsle\deskisle.exe')

function Start-UpdateProcess([string]$file, [string]$arguments) {
    $info = [Diagnostics.ProcessStartInfo]::new($file, $arguments)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.WorkingDirectory = [IO.Path]::GetDirectoryName($file)
    [Diagnostics.Process]::Start($info)
}

function Wait-ForImageRelease {
    # The taskbar recovery process can still map the image after the Dock exits.
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ([IO.File]::Exists($target)) {
        try {
            [IO.File]::Open($target, 'Open', 'Write', 'ReadWrite, Delete').Dispose()
            return
        } catch {
            $code = $_.Exception.GetBaseException().HResult -band 0xffff
            if ($code -notin 5, 32, 33, 1224 -or $timer.Elapsed.TotalSeconds -ge 5) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}

function Invoke-DeskIsleUpdate {
    # Open the process handle before acknowledging readiness to prevent PID reuse.
    $parent = [Diagnostics.Process]::GetProcessById($parentId)
    $null = $parent.Handle
    [Console]::WriteLine('ready')
    try {
        $parent.WaitForExit()
        Wait-ForImageRelease
        $setup = Start-UpdateProcess $payload '/S'
        try {
            $setup.WaitForExit()
            if ($setup.ExitCode -ne 0) {
                throw ('安装包退出，错误码 ' + $setup.ExitCode + '。')
            }
        } finally {
            $setup.Dispose()
        }
        (Start-UpdateProcess $target '').Dispose()
    } finally {
        $parent.Dispose()
        [IO.File]::Delete($payload)
        [IO.Directory]::Delete([IO.Path]::GetDirectoryName($payload))
    }
}

function Start-UpdateHelper {
    try { Invoke-DeskIsleUpdate } catch {
        $message = '更新失败。' + $_.Exception.GetBaseException().Message
        Add-Type -AssemblyName System.Windows.Forms
        [void][Windows.Forms.MessageBox]::Show($message, 'DeskIsle 更新')
        exit 1
    }
}
