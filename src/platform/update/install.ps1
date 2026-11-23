$ErrorActionPreference = 'Stop'
$target = $env:DESKISLE_UPDATE_EXE
$stage = $env:DESKISLE_UPDATE_DIR
$payload = [IO.Path]::Combine($stage, 'deskisle.exe')
$backup = [IO.Path]::Combine($stage, 'previous.exe')

function Start-DeskIsle {
    $info = [Diagnostics.ProcessStartInfo]::new($target)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.WorkingDirectory = [IO.Path]::GetDirectoryName($target)
    $info.EnvironmentVariables.Remove('DESKISLE_UPDATE_EXE')
    $info.EnvironmentVariables.Remove('DESKISLE_UPDATE_DIR')
    $info.EnvironmentVariables.Remove('DESKISLE_UPDATE_PID')
    [Diagnostics.Process]::Start($info).Dispose()
}

function Install-DeskIsleUpdate {
    try {
        # The taskbar recovery process maps the same executable and may still
        # be exiting. Retry only transient access/sharing failures.
        $timer = [Diagnostics.Stopwatch]::StartNew()
        while ($true) {
            try {
                [IO.File]::Replace($payload, $target, $backup)
                break
            } catch {
                $code = $_.Exception.GetBaseException().HResult -band 0xffff
                # A partial replacement must be rolled back before another attempt.
                if ([IO.File]::Exists($backup)) { throw }
                if ($code -notin 5, 32, 33, 1224 -or $timer.Elapsed.TotalSeconds -ge 5) { throw }
                Start-Sleep -Milliseconds 100
            }
        }
        Start-DeskIsle
    } catch {
        $installError = $_.Exception.GetBaseException().Message
        if ([IO.File]::Exists($backup)) {
            try {
                if ([IO.File]::Exists($target)) {
                    [IO.File]::Replace($backup, $target, [IO.Path]::Combine($stage, 'failed.exe'))
                    [IO.File]::Delete([IO.Path]::Combine($stage, 'failed.exe'))
                } else {
                    # ReplaceFile can move the original to backup before failing.
                    [IO.File]::Move($backup, $target)
                }
            } catch {
                throw ('更新失败：' + $installError + '。无法还原旧版，请检查 ' + $stage + '。' + $_.Exception.GetBaseException().Message)
            }
        }
        try { Start-DeskIsle } catch {
            throw ('更新失败：' + $installError + '。无法重新启动 ' + $target + '。' + $_.Exception.GetBaseException().Message)
        }
        throw ('更新失败，已重新启动原版本。' + $installError)
    }
    [IO.File]::Delete($backup)
}

function Invoke-DeskIsleUpdate {
    # Opening the handle before acknowledging readiness also prevents PID reuse.
    $parent = [Diagnostics.Process]::GetProcessById([int]$env:DESKISLE_UPDATE_PID)
    $null = $parent.Handle
    [Console]::WriteLine('ready')
    try {
        $parent.WaitForExit()
        Install-DeskIsleUpdate
    } finally {
        $parent.Dispose()
        [IO.File]::Delete($payload)
        # Leave the backup available if restoring it also failed.
        if (![IO.File]::Exists($backup)) { [IO.Directory]::Delete($stage) }
    }
}

function Start-UpdateHelper {
    try { Invoke-DeskIsleUpdate } catch {
        $message = $_.Exception.GetBaseException().Message
        Add-Type -AssemblyName System.Windows.Forms
        [void][Windows.Forms.MessageBox]::Show($message, 'DeskIsle 更新')
        exit 1
    }
}
