# 用 FlashLink 从安卓连接 Windows

在 Windows 电脑上运行主机应用，在安卓手机或平板上打开 FlashLink，就能从「我的设备」选择桌面。发现、首次授权和再次连接都在局域网内完成。

## 准备两端应用

| 设备 | 系统要求 | 试玩包中的文件 |
| --- | --- | --- |
| 被控电脑 | Windows 10/11 x64，已登录且桌面解锁 | `FlashLink-Windows-x64.exe` |
| 手机或平板 | Android 7.0 及以上，arm64 | `FlashLink-Android-arm64.apk` |

已有试玩包时，分别把 EXE 和 APK 放到对应设备上。Windows 单文件应用内含 Sunshine，安卓应用内含 Moonlight。若使用 Windows 便携 ZIP，请先完整解压，再双击其中的 `FlashLink.exe`。从 Git 克隆项目的开发者可以按下方步骤生成安装包。

## 首次连接

1. 在电脑上双击 EXE，等窗口显示「已就绪，等待手机或平板连接」，保持主机程序运行。
2. 在安卓上安装并打开 APK，连接与电脑同一局域网的 Wi-Fi。电脑也可以用网线连接路由器。看到电脑名称后，点在线设备卡，或从详情点「进入桌面」。
3. 电脑会弹出连接请求。确认发起请求的是自己的安卓设备，并核对两端显示的**完整确认码**。一致时点 Windows 上的「允许连接」，安卓就会进入桌面。

下次打开两端应用，点同一张设备卡即可直连。主窗口关闭后会驻留托盘；点托盘图标可重新打开。要停止服务，选「主机 → 退出主机」。在「管理已授权设备」中选择设备并确认，可立即结束该设备的连接并移除授权。

## 串流中的操作

在远程桌面顶部点「鼠标」展开触控板、左中右键和上下滚动。点「拖动」后移动指针，再点一次释放。点「工具」可以发送空格、回车等快捷键，输入一段文字，或主动断开。面板收起后可以继续直接点游戏画面。

正在玩的会话因网络异常中断时，安卓自动尝试连接同一台已授权电脑，最长尝试 15 秒。超时后可回设备列表重试。手动断开会直接退出。Windows 在普通模式下继续显示本地桌面。按 Win+L 会暂停串流和附近发现，本机解锁后主机会自动重新上线，保留原来的配对。

## 隐私黑屏（真机待验收）

1. 在 Windows 打开「主机 → 安装虚拟屏…」，按系统提示完成随包组件的安装。如果系统要求重启，重启后再连接。
2. 安卓先进入远程桌面。当前试玩版开启隐私时要求只有一台启用了授权的客户端，并且电脑处于已解锁状态。
3. 在 Windows 主机菜单选「切换隐私模式」。也可以在安卓串流的桌面里操作同一个 Windows 窗口。虚拟屏按开启前 Windows 主屏的桌面分辨率接管，单屏和扩展桌面都使用这个规则。主机会记录快捷方式位置，并在虚拟桌面上校正原主屏的图标。若虚拟驱动不支持该分辨率，开启前会提示；使用 FlashLink 安装的虚拟屏可再次运行「安装虚拟屏」补充当前屏幕模式。若主机已收起，点任务栏通知区的 FlashLink 图标打开。看到开启提示后，对照平板及各物理屏确认实际效果。
4. 隐私期间掉线，主机会为原设备保留短时重连机会。手机在线时再次点击切换会尝试恢复原布局并继续串流。系统短暂重新启用已知实体屏时，主机会立即重新应用虚拟屏配置，重新关闭输出前实体屏可能短暂可见。实体屏反复重新上线时，主机自动退出隐私，恢复原显示布局并尝试继续已授权的串流。此时本机可以看到当前桌面。断线超时或主程序异常退出时，守护进程结束串流并尝试恢复显示。若恢复仍在等待，可在本机已解锁且串流已停止时选择「主机 → 本机恢复显示…」。若所有物理屏都不可见，重启电脑可让 Windows 重新应用已保存的显示配置。此次没有实体 Windows 验证，首次试用请用普通桌面内容检查。

## 画质设置和连接问题

在安卓「设置 → 播放设置」可调整分辨率、帧率、码率并查看性能统计。首次使用时，2560×1440 及以上的屏幕默认请求 1440p/60 FPS，1920×1080 级屏幕请求 1080p/60 FPS，更小的屏幕请求 720p/60 FPS。可根据小字清晰度、语音连续性和实际渲染帧率调整。

| 现象 | 操作 |
| --- | --- |
| Windows 显示「主机未就绪」 | 查看窗口里的启动错误，必要时点「查看日志」。确认旧 FlashLink 主机窗口已退出后，点「重新启动」。 |
| Windows 已就绪，安卓上看不到电脑 | 确认两端处于允许设备互访的同一局域网。在可信的家庭网络上，可点 Windows 中的「允许专用网络连接」，确认系统授权后在安卓页点刷新。访客 Wi-Fi 的设备隔离可能阻止发现。 |
| 点击连接后一直等待 | 查看 Windows 的授权弹窗。请求已超时或被拒绝时，回到设备列表重新点连接。若两边确认码不一致，点「拒绝」。 |
| 已连接但画面或声音异常 | 先在电脑本机确认显示、音频输出正常，再从 Windows「查看日志」检查采集、编码器和音频设备。主机日志位于 `%LOCALAPPDATA%\FlashLink\DemoHost\flashlink-sunshine.log`。 |

## 在 Mac 测试和导出数据

在仓库根目录运行以下命令。需要 Python 3.8 或更新版本，以及项目使用的 Rust、Flutter SDK 和 JDK。可以通过 `FLUTTER_BIN` 指定 Flutter 可执行文件，通过 `JAVA_HOME` 指定 JDK。报告会记录缺少的工具。

```sh
python3 test/test_flashlink.py --preview
```

这个命令运行 Rust、Flutter 和 Android 重连策略测试，将一份带时间的 JSON 写入 `testdata/`，将手机、平板的示例界面写入 `testdata/ui-preview/`。预览使用测试设备数据。单元测试无需 Windows、安卓设备或模拟器。

将开过串流的安卓设备通过 USB 接到 Mac，允许 USB 调试，在 Mac 准备好 Android Platform Tools 后，运行以下命令。它读取最近串流的重连事件、接收和提交到 Surface 的帧数、丢帧及解码耗时。静态桌面上的数字受画面变化影响，评估 60/120 FPS 请播放有持续运动的素材。可以通过环境变量 `ADB` 指定可执行文件；多设备时用 `ANDROID_SERIAL` 选择设备。

```sh
python3 test/test_flashlink.py --mode field --android
```

**Windows 试玩者可直接在「主机 → 导出测试报告…」保存数据**，文件在系统文稿文件夹下的 `FlashLink/testdata/`。从源码运行则可在仓库根目录执行 `.\test\Test-FlashLink.ps1`，默认写入仓库的 `testdata/`。该报告包含显卡型号、驱动、系统桌面模式、当前及上次有串流日志的事件计数、分类告警与错误、隐私状态变化。电脑端与安卓端的报告可以一起带回 Mac，工具只写本地文件。

要补记首句声音、触控和每块显示器实际表现，在 Python 命令中添加 `--mode field --observe`；直接回车会保留「未测试」。`dev` 返回 0 代表自动检查通过；`field` 返回 0 代表取得数据或观察，实体屏验收以观察字段为准。返回 1 表示记录到了失败，2 表示此次未采集到所请求的数据或缺少开发工具。[测试入口](../test/test_flashlink.py)

## 从源码生成试玩包

以下命令均从仓库根目录执行。首次构建需要联网获取上游源码和构建依赖。

### Windows 被控端

在 Windows x64 上准备 Rust 1.89 或更新版本（MSVC 工具链）、Visual Studio C++ 构建工具和 Python 3。下载固定版本的 `Sunshine-Windows-AMD64-lite.zip`，将同一版本的许可证另存为 `Sunshine-LICENSE`。以下命令假定这两个文件位于当前用户的 Downloads 目录。[Sunshine 发行包](https://github.com/SunshineStream/Sunshine/releases/tag/v2026.914.233613) · [对应许可证原文](https://raw.githubusercontent.com/SunshineStream/Sunshine/v2026.914.233613/LICENSE)

```powershell
cargo build --locked --release -p flashlink-host
python packaging/windows/bundle.py --exe target/release/flashlink-host.exe --sunshine-zip "$env:USERPROFILE\Downloads\Sunshine-Windows-AMD64-lite.zip" --sunshine-license "$env:USERPROFILE\Downloads\Sunshine-LICENSE"
```

打包脚本核验 Sunshine 的 SHA-256 后生成 `dist/FlashLink-Windows-x64.exe` 和 `dist/FlashLink-Windows-x64.zip`。

### Android 控制端

试玩版使用 Flutter 3.38.9、JDK 17、Rust、Python 3 和 Git。Android SDK 需包含 Platform 36、Build Tools 35.0.0 和 36.0.0、NDK 28.2.13676358、CMake 3.22.1。设置 `ANDROID_HOME` 和 `JAVA_HOME`，并确保可以调用 `flutter`。

```sh
rustup target add aarch64-linux-android
python3 scripts/android_demo.py
```

把生成的 `build/android-demo/flashlink-arm64-debug.apk` 安装到安卓设备。可在系统的应用信息中核对版本 `0.3.0-demo`（316）。[Android 构建脚本](../scripts/android_demo.py)

## 验证范围

已在 Android 14 arm64 模拟器上验证自动发现、允许和拒绝首次请求，以及覆盖安装后的直接重连。实际串流测试使用隔离的 Linux Sunshine 主机，安卓收到 1280×720 桌面画面，触屏点击回传到了主机。Windows x64 产物已完成交叉编译和打包检查。新版 APK 在同一测试环境通过了实际左右中键、滚轮、空格和回车回传。一次短时断网从检测到恢复约 7 秒，持续断网则在检测后约 15 秒停止尝试；主动退出没有触发重连。已配对的安卓播放器通过了与 Windows 共用的 Rust 心跳认证。物理 Windows 电脑上的 GPU 编码、声音稳定性、隐私黑屏与显示恢复、1440p/60 和 120 FPS 尚待真机验证。
