# 用 FlashLink 从安卓连接 Windows

在 Windows 电脑上运行主机应用，在安卓手机或平板上打开 FlashLink，就能从「附近的电脑」选择桌面。发现、首次授权和再次连接都在局域网内完成。

## 准备两端应用

| 设备 | 系统要求 | 试玩包中的文件 |
| --- | --- | --- |
| 被控电脑 | Windows 10/11 x64，已登录且桌面解锁 | `FlashLink-Windows-x64.exe` |
| 手机或平板 | Android 7.0 及以上，arm64 | `FlashLink-Android-arm64.apk` |

已有试玩包时，分别把 EXE 和 APK 放到对应设备上。Windows 单文件应用内含 Sunshine，安卓应用内含 Moonlight。若使用 Windows 便携 ZIP，请先完整解压，再双击其中的 `FlashLink.exe`。从 Git 克隆项目的开发者可以按下方步骤生成安装包。

## 首次连接

1. 在电脑上双击 EXE，等窗口显示「已就绪，等待手机或平板连接」，并保持窗口打开。
2. 在安卓上安装并打开 APK，连接与电脑同一局域网的 Wi-Fi。电脑也可以用网线连接路由器。看到电脑名称后，点设备卡上的「连接电脑」。
3. 电脑会弹出连接请求。确认发起请求的是自己的安卓设备，并核对两端显示的**完整确认码**。一致时点 Windows 上的「允许连接」，安卓就会进入桌面。

下次打开两端应用，点同一张设备卡即可直连。关闭 Windows 主机窗口会停止服务，配对记录会保留。**试玩期间，Windows 的物理显示器仍会显示桌面。**

## 画质设置和连接问题

在安卓首页点「播放设置」可调整分辨率、帧率、码率并查看性能统计。首次使用时，2560×1440 及以上的屏幕默认请求 1440p/60 FPS，1920×1080 级屏幕请求 1080p/60 FPS，更小的屏幕请求 720p/60 FPS。可根据小字清晰度、语音连续性和实际渲染帧率调整。

| 现象 | 操作 |
| --- | --- |
| Windows 显示「主机未就绪」 | 查看窗口里的启动错误，必要时点「查看日志」。确认旧 FlashLink 主机窗口已退出后，点「重新启动」。 |
| Windows 已就绪，安卓上看不到电脑 | 确认两端处于允许设备互访的同一局域网。在可信的家庭网络上，可点 Windows 中的「允许专用网络连接」，确认系统授权后在安卓页点刷新。访客 Wi-Fi 的设备隔离可能阻止发现。 |
| 点击连接后一直等待 | 查看 Windows 的授权弹窗。请求已超时或被拒绝时，回到设备列表重新点连接。若两边确认码不一致，点「拒绝」。 |
| 已连接但画面或声音异常 | 先在电脑本机确认显示、音频输出正常，再从 Windows「查看日志」检查采集、编码器和音频设备。主机日志位于 `%LOCALAPPDATA%\FlashLink\DemoHost\flashlink-sunshine.log`。 |

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

把生成的 `build/android-demo/flashlink-arm64-debug.apk` 安装到安卓设备。可在系统的应用信息中核对版本 `0.2.0-demo`（315）。[Android 构建脚本](../scripts/android_demo.py)

## 验证范围

已在 Android 14 arm64 模拟器上验证自动发现、允许和拒绝首次请求，以及覆盖安装后的直接重连。实际串流测试使用隔离的 Linux Sunshine 主机，安卓收到 1280×720 桌面画面，触屏点击回传到了主机。Windows x64 产物已完成交叉编译和打包检查。物理 Windows 电脑上的 GPU 编码、声音稳定性、1440p/60 和 120 FPS 尚待真机验证。
