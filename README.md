# FlashLink

FlashLink 让安卓手机和平板通过局域网连接 Windows 桌面，适合在平板上看清游戏文字、立绘并听到电脑的声音。Windows 端内置 Sunshine，Android 端集成 Moonlight。1.0 正在开发中。

## 开始使用

1. 在已登录且已解锁的 Windows 10/11 x64 电脑上运行 `FlashLink-Windows-x64.exe`。
2. 在 Android 7 或更新版本的 arm64 设备上安装 APK，连到与电脑相同的局域网，打开「我的设备」。
3. 点选电脑，首次连接时核对两端的确认码，并在 Windows 上允许连接。之后再点同一台电脑即可直连。

安卓端目前可以通过浮动触控板移动鼠标、点击和滚动，也可以发送按键或文本。直接点按远程坐标、双指缩放与平移分别由 [#6](https://github.com/RicardoMin/FlashLink/issues/6) 和 [#8](https://github.com/RicardoMin/FlashLink/issues/8) 跟踪。

连接故障、隐私模式、诊断报告和两端的构建命令见[使用说明](docs/usage.md)。

## 源码

| 目录 | 职责 |
| --- | --- |
| [apps/flashlink](apps/flashlink) | Flutter 设备列表、主机详情与连接界面 |
| [apps/android](apps/android) | Android 发现与授权、Moonlight 播放器集成和 APK 构建 |
| [crates/client](crates/client) | Flutter 通过 FFI 调用的 Rust 会话状态机 |
| [crates/host-windows](crates/host-windows) | Windows 主机、Sunshine 进程管理、授权、显示恢复和 Windows 分发包 |
| [test](test) | 本地开发检查与真机诊断报告入口 |

协议流程、模块边界和代码入口见[项目架构](docs/architecture.md)。已知问题与开发中的功能见 [GitHub Issues](https://github.com/RicardoMin/FlashLink/issues)。

## 许可

FlashLink 自有代码采用 [MIT 许可](LICENSE)。Android 集成了 GPLv3 的 Moonlight，Windows 包含独立运行的 Sunshine 和虚拟屏驱动。两端保留各自的第三方声明及对应源码信息。[Android 声明](apps/android/NOTICE.txt) · [Windows 声明](crates/host-windows/assets/NOTICE.txt)
