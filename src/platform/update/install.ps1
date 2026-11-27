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

function New-UpdateFailureWindow([string]$detail) {
    # The Dock and its renderer have exited by the time the installer runs.
    Add-Type -AssemblyName PresentationFramework
    $view = @'
<Window xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation'
        xmlns:x='http://schemas.microsoft.com/winfx/2006/xaml'
        Title='DeskIsle 更新' WindowStyle='None' AllowsTransparency='True' Background='Transparent'
        ResizeMode='NoResize' WindowStartupLocation='CenterScreen' Width='384' SizeToContent='Height'
        FontFamily='Segoe UI, Microsoft YaHei UI' UseLayoutRounding='True'>
    <Border Margin='8' CornerRadius='18' Padding='24' BorderThickness='1'
            Background='{DynamicResource Surface}' BorderBrush='{DynamicResource Edge}'>
        <Border.Effect><DropShadowEffect BlurRadius='12' ShadowDepth='2' Opacity='0.21'/></Border.Effect>
        <StackPanel>
            <DockPanel>
                <Button Name='Close' DockPanel.Dock='Right' Width='28' Height='28' AutomationProperties.Name='关闭' ToolTip='关闭'>
                    <Button.Template>
                        <ControlTemplate TargetType='Button'>
                            <Border Name='Hit' CornerRadius='7' Background='Transparent'>
                                <Path Data='M 1,1 L 10,10 M 10,1 L 1,10' Stroke='{DynamicResource Muted}' StrokeThickness='1.4'
                                      HorizontalAlignment='Center' VerticalAlignment='Center'/>
                            </Border>
                            <ControlTemplate.Triggers>
                                <Trigger Property='IsMouseOver' Value='True'><Setter TargetName='Hit' Property='Background' Value='{DynamicResource Edge}'/></Trigger>
                            </ControlTemplate.Triggers>
                        </ControlTemplate>
                    </Button.Template>
                </Button>
                <TextBlock Name='Heading' Text='DeskIsle 软件更新' Background='Transparent' VerticalAlignment='Center'
                           Foreground='{DynamicResource Muted}' FontSize='13'/>
            </DockPanel>
            <DockPanel Margin='0,20,0,20'>
                <Border Width='48' Height='48' CornerRadius='14' VerticalAlignment='Top' Background='{DynamicResource IconSurface}'>
                    <Path Data='M 12,1 L 23,21 L 1,21 Z M 12,7 L 12,13 M 12,15 L 12,17'
                          Stroke='{DynamicResource IconInk}' StrokeThickness='2' StrokeLineJoin='Round'
                          HorizontalAlignment='Center' VerticalAlignment='Center'/>
                </Border>
                <StackPanel Margin='14,0,0,0'>
                    <TextBlock Text='更新未完成' Margin='0,0,0,5' Foreground='{DynamicResource Ink}'
                               FontSize='20' FontWeight='SemiBold'/>
                    <ScrollViewer MaxHeight='136' VerticalScrollBarVisibility='Auto'>
                        <TextBlock Name='Detail' TextWrapping='Wrap' Foreground='{DynamicResource Muted}' FontSize='14'/>
                    </ScrollViewer>
                </StackPanel>
            </DockPanel>
            <Border Height='1' Background='{DynamicResource Edge}'/>
            <Button Name='Dismiss' Content='关闭' Margin='0,14,0,0' MinWidth='88' Height='34'
                    HorizontalAlignment='Right' Foreground='White' IsCancel='True' IsDefault='True'>
                <Button.Template>
                    <ControlTemplate TargetType='Button'>
                        <Border Name='Face' Background='#3559B7' CornerRadius='8'>
                            <ContentPresenter HorizontalAlignment='Center' VerticalAlignment='Center'/>
                        </Border>
                        <ControlTemplate.Triggers>
                            <Trigger Property='IsMouseOver' Value='True'><Setter TargetName='Face' Property='Background' Value='#294BA6'/></Trigger>
                            <Trigger Property='IsPressed' Value='True'><Setter TargetName='Face' Property='Background' Value='#203F91'/></Trigger>
                        </ControlTemplate.Triggers>
                    </ControlTemplate>
                </Button.Template>
            </Button>
        </StackPanel>
    </Border>
</Window>
'@
    $window = [Windows.Markup.XamlReader]::Parse($view)
    $themeKey = 'HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
    $colors = if ([Microsoft.Win32.Registry]::GetValue($themeKey, 'AppsUseLightTheme', 1) -eq 0) {
        @{ Surface='#22252B'; Edge='#48505A'; Ink='#F6F7FA'; Muted='#C2C9D5'; IconSurface='#4C3034'; IconInk='#FFA6A9' }
    } else {
        @{ Surface='#FAFBFD'; Edge='#D8DDE5'; Ink='#1C2535'; Muted='#526075'; IconSurface='#FFE9E9'; IconInk='#AD3846' }
    }
    foreach ($key in $colors.Keys) { $window.Resources[$key] = [Windows.Media.Brush]$colors[$key] }
    $window.MaxWidth = [Windows.SystemParameters]::WorkArea.Width
    $window.FindName('Detail').Text = $detail
    $close = { [Windows.Window]::GetWindow($this).Close() }
    $window.FindName('Close').Add_Click($close)
    $window.FindName('Dismiss').Add_Click($close)
    $window.FindName('Heading').Add_MouseLeftButtonDown({ [Windows.Window]::GetWindow($this).DragMove() })
    $window
}

function Start-UpdateHelper {
    try { Invoke-DeskIsleUpdate } catch {
        $window = New-UpdateFailureWindow $_.Exception.GetBaseException().Message
        [void]$window.ShowDialog()
        exit 1
    }
}
