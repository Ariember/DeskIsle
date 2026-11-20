# DeskIsle 开发选型

DeskIsle 是面向 Windows 的桌面美化工具。首个开发目标是常驻 Dock，支持应用启动、窗口切换和悬停动效。选型按稳定性、运行性能、视觉效果的顺序取舍。首要验证环境为 Windows 11 x64。

## 技术栈

| 范围 | 选择 | 选择理由与代价 |
| --- | --- | --- |
| 语言与构建 | Rust stable、Cargo、MSVC 工具链 | 所有权适合管理图标、句柄和图形资源的释放。Windows 回调的线程约束和外部窗口的有效性仍需由代码保证。开发机使用 Visual Studio C++ 生成工具及 Windows SDK。[Rust MSVC 平台说明](https://doc.rust-lang.org/rustc/platform-support/windows-msvc.html) |
| 系统集成 | 微软维护的 `windows` crate，调用 Win32 和 Shell API | 同一套绑定可访问消息循环、窗口、显示器、程序启动和图形接口。按实际调用启用 crate 特性；创建应用工程时提交 `Cargo.lock`，使构建使用同一组依赖。[Rust for Windows](https://github.com/microsoft/windows-rs) |
| Dock 渲染 | DirectComposition 合成，Direct3D 11 提供图形表面 | 图标上传后可重复使用，悬停时更新缩放和位移，由系统驱动过渡动画。此方案需要自行实现命中测试、键盘操作和无障碍语义，并处理图形设备重建。[DirectComposition 概览](https://learn.microsoft.com/en-us/windows/win32/directcomp/directcomposition-portal) |
| 桌面集成 | 以普通用户权限运行独立桌面进程 | Dock 在主屏临时隐藏 Explorer 任务栏，保留用户的自动隐藏设置。恢复进程通过管道感知 Dock 退出，主进程被强制终止后也可还原原生任务栏。 |
| 系统控制 | Core Audio、`IPolicyConfig`、Power、Shutdown（睡眠、重启、关机、注销与锁屏）、DDC/CI 与 DisplayConfig；WinRT 的 `Windows.Devices.Radios`、`Windows.Networking.Connectivity` 和 `Windows.Media.Control` | 这些接口都通过已有的 `windows` 依赖调用，无需安装额外运行时。WinRT 异步操作在 UI 线程上用 `join` 等待结果，只在用户打开面板或执行操作时调用。COM 回调和自定义接口需要 `implement`、`interface` 宏，因此直接依赖 `windows-core`，其版本已在 `Cargo.lock` 中。代价是 `IPolicyConfig` 没有公开文档，亮度只支持 DDC/CI 显示器。 |
| 状态栏与托盘 | `SHAppBarMessage` 预留顶部条带；同类名 `Shell_TrayWnd` 宿主接收并转发托盘通知 | 只有 AppBar 能让最大化窗口避开全宽状态栏。Windows 11 的托盘由 XAML 绘制，旧的工具栏读取方式已失效，接管通知是获取图标的可行途径。代价是其他按类名查找任务栏的工具可能先找到 DeskIsle 的窗口，因此宿主使用顶部栏矩形，回答已布局图标的位置查询，并把其余任务栏请求转发给 Explorer。进程意外终止时，Explorer 可能暂时保留预留的条带。[AppBar 说明](https://learn.microsoft.com/en-us/windows/win32/shell/application-desktop-toolbars) |

## 如何验证选型

首版两栏和右键菜单共用 D3D11 和 DirectComposition 设备，使用 Direct2D 绘制表面、DirectWrite 排印文字，并通过 WIC 解码 Shell 图标。界面图标不再依赖 Segoe Fluent Icons 字体，而由 Direct2D 路径几何按 `ui/icon.rs` 的轮廓绘制：图标风格可以自行统一，也不受 Windows 10 未自带该字体的限制；代价是每个新图标都要在代码里设计轮廓。这些接口通过同一个 `windows` 依赖调用。悬停动画由合成器执行，时钟在分钟变化时更新。

在 Windows x64 的 MSVC 开发环境运行 `cargo run --release --locked` 启动原型。运行 `cargo test --locked` 检查状态与布局。在交互式桌面运行 `cargo test --locked --test windows_bars -- --ignored` 可验证启动前已最大化和运行中重新最大化的窗口边界、双栏位置，以及正常退出和强制结束后的工作区恢复。

- **稳定性**：覆盖启动与退出、用户选中窗口前目标已关闭、最小化恢复、资源管理器重启、睡眠唤醒，以及不同缩放比例下的显示器热插拔。以实际前台窗口、可操作的系统桌面和正确的 Dock 位置作为验收依据。
- **性能**：在发布构建上分别测静置 CPU、工作集和连续悬停时的可见更新间隔。记录显卡、刷新率、缩放比例、采样时长及测量方法。判断高刷表现要看帧间隔分布；桌面同步等待耗时只能说明同步节拍。
- **视觉**：在目标显示器上检查图标清晰度、不同背景上的辨识度，以及放大时的点击位置。增加模糊、阴影等效果前后，用相同场景比较帧时间和闲置占用。

模块职责、事件流和资源恢复约定见[架构规范](architecture.md)。
