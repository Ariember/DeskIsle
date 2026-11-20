# DeskIsle

DeskIsle 是面向 Windows 11 的桌面工具，在主屏提供半透明的顶部状态栏和底部 Dock。Dock 用于启动应用和切换窗口，状态栏集中显示应用托盘图标、系统状态和时间。

## 运行

使用 Windows 11 x64、Rust stable 的 MSVC 工具链、Visual Studio C++ 生成工具和 Windows SDK。在仓库根目录运行以下命令。

```powershell
cargo run --release --locked
```

只需构建时，运行 `cargo build --release --locked`。生成的程序位于 `target\release\deskisle.exe`。

DeskIsle 以普通用户权限运行。默认在运行期间隐藏主屏的 Windows 任务栏，退出时恢复。可在设置中关闭“隐藏系统任务栏”。

## 使用

- **Dock**。左键启动应用或切换到已打开的窗口。应用有多个窗口时，先从列表选择。中键新开一个实例。右键可固定或取消固定应用，也可打开文件位置或关闭窗口。
- **顶部状态栏**。单击系统图标可打开对应面板，查看或调整网络、声音、显示和电源等设置；右键直接打开对应的 Windows 设置页。在声音图标上滚动滚轮可调节音量，中键切换静音。电池图标旁显示电量百分比。控制中心还提供常用开关和媒体控制。
- **系统菜单**。单击顶部栏最左侧的圆环标志，可打开关于本机、Windows 设置、应用商店和任务管理器，或让电脑睡眠、重新启动、关机、锁定屏幕和注销。重新启动、关机和注销不强制关闭应用，有未保存工作的应用仍可提示。
- **托盘**。点击展开箭头可访问其余应用，并在“在状态栏显示”中勾选要常驻的图标。扬声器、安全删除硬件等 Windows 系统图标会自动过滤。DeskIsle 检测到应用提醒时，会在图标旁显示静态圆点。左键点击会清除该圆点，并尝试打开或切回应用主窗口，无法打开时尝试显示应用菜单。右键可直接打开应用菜单。
- **设置与退出**。右键顶部栏标志，或点击栏空白处打开菜单，选择“设置”或“退出 DeskIsle”。设置中可切换浅色、深色和跟随系统，调整 Dock 尺寸与悬停放大，或启用开机自动启动。

亮度调节需要显示器支持 DDC/CI。在支持现代待机的电脑上，点击“睡眠”会请求关闭屏幕，随后由 Windows 按电源策略进入低功耗状态。托盘菜单由所属应用绘制，菜单位置和可用操作取决于应用。以管理员权限运行的应用可能无法接收 DeskIsle 转发的托盘点击。

## 本地配置

配置保存在 `%APPDATA%\DeskIsle\`。其中 `settings.ini` 保存外观与行为设置，`pins.txt` 保存固定应用，`tray-hidden.txt` 保存隐藏的托盘图标。开机自动启动状态由当前用户的 Windows 注册表启动项管理。

## 开发

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

在交互式 Windows 桌面上，可运行 `cargo test --locked --test windows_bars -- --ignored` 验证双栏位置和工作区恢复。

- [开发选型](docs/technology.md)
- [架构规范](docs/architecture.md)
- [开发规范](AGENTS.md)
