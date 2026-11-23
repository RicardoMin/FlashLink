# 用安卓手机或平板连接 Windows

[Windows x64 单文件应用](dist/FlashLink-Windows-x64.exe) · [Android arm64 安装包](dist/FlashLink-Android-arm64.apk) · [Windows 便携包](dist/FlashLink-Windows-x64.zip) · [安卓手机界面](dist/FlashLink-Android-phone.png) · [安卓平板界面](dist/FlashLink-Android-home.png)

## 第一次连接

1. **在电脑上**双击 `FlashLink-Windows-x64.exe`，等待主机窗口显示「已就绪」。首次运行会在当前用户下准备随包提供的 Sunshine 引擎。保持窗口打开、Windows 桌面登录并解锁。
2. **在安卓手机或平板上**安装 APK，打开 FlashLink。手机连 Wi-Fi，电脑可以接网线，只要在同一个允许设备互访的局域网内。「我的设备 → 可控设备」会自动显示 Windows 电脑名，点在线设备卡进入桌面。
3. **在电脑上允许**。核对两个应用显示的完整确认码和设备名称，点击 Windows 弹窗中的「允许连接」。手机随即打开 Windows 桌面，可以进入游戏。以后同一台安卓设备再点电脑卡即可直连。点击「拒绝」会结束本次请求。

这个版本的连接和授权都在两个应用内完成。旧版使用网页完成的首次授权，已移到 Windows 窗口。电脑同时运行其他 Sunshine 时，FlashLink 会尝试选择可用的串流端口，安卓从发现应答取得实际端口。

普通模式下物理屏显示游戏画面。Windows 菜单可以安装内置虚拟屏组件并开启隐私，系统托盘可以管理已授权设备、导出报告或退出主机。当前交付面向 Windows 10/11 x64 被控端和 Android 7.0 及以上的 arm64 手机、平板。隐私模式的条件、断线行为和 Mac 上的测试方法见[使用说明](docs/usage.md)。

## 遇到问题

| 现象 | 查看位置和操作 |
| --- | --- |
| Windows 显示「主机未就绪」 | 查看同一窗口的启动错误，必要时点「查看日志」。确认已退出上次运行的 FlashLink，再点「重新启动」。如果使用 ZIP，请先解压整个包，再运行其中的 `FlashLink.exe`。 |
| Windows 已就绪，安卓列表一直为空 | 首先确认手机连 Wi-Fi、电脑与手机处在同一网络。在可信的家庭网络，可点 Windows 应用内的「允许专用网络连接」，确认系统授权后在手机点刷新。规则仅允许指定程序和端口接受本地子网的专用网络连接。访客 Wi-Fi、客户端隔离或不同 VLAN 会妨碍发现。 |
| 看到设备，但首次连接未完成 | 查看 Windows 是否弹出授权窗口。两端确认码一致时才允许。请求被拒绝或超时后，回到手机设备列表再次点击。重新安装客户端、清除应用数据或主机证书变化时，需要重新授权。 |
| 已进入桌面，画面或声音异常 | 确认 Windows 的显示器和音频输出处于工作状态。电脑点「查看日志」检查采集、编码器和音频设备；手机点「播放设置」查看分辨率、帧率、码率及性能统计。 |

## 画质与声音

初装时，2560×1440 级或更高分辨率的屏幕默认请求 1440p/60 FPS，1920×1080 级请求 1080p/60 FPS，更小的屏幕使用 720p/60 FPS。可在「设置 → 播放设置」调整，后续保留用户选择。Windows 当前桌面分辨率仍会影响文字清晰度。

用同一段对话、语音和转场检查小字、首句声音、声道和触控翻页。高刷测试应同时开启系统高刷新率与播放器 120 FPS，观看有持续运动的素材，并检查实际接收和渲染帧率。

## 实现与构建

| 代码 | 负责的工作 |
| --- | --- |
| `apps/flashlink`、`crates/client` | Flutter 设备列表、刷新与页面生命周期，Rust 校验目标、管理单个活动连接，防止重复打开和过期回调。 |
| `crates/host-windows` | Rust 原生 Windows 窗口、Sunshine 监督、就绪探测、附近发现、首次授权与防火墙规则。状态、证书和引擎日志在当前用户的 `%LOCALAPPDATA%\FlashLink\DemoHost`。 |
| `packaging/android` | Android 在 Wi-Fi 或以太网上发现 FlashLink 主机。首次连接用主机证书固定的 TLS 通道提交临时请求，由 Moonlight 执行证书配对。之后按已有证书连接，查询 Desktop 的实际应用 ID；音视频与输入使用上游原生管线。 |
| `packaging/windows`、`scripts/android_demo.py` | 生成含固定版 Sunshine 的 Windows 应用，以及含 Flutter、Rust 和 Moonlight 的单一 APK。 |

发现协议使用 IPv4 UDP 47980，每次搜索发送新的随机值。主机应答包含本次随机值、电脑名、实际串流端口和证书 SHA-256。客户端只向响应报文的来源地址发起连接。首次授权通道使用 TCP 串流基准端口减 8。Windows 同时匹配 Sunshine 的待配对记录、来源地址和一次性请求编号，在用户允许后通过仅本机可用的接口确认该次请求。配对记录由 Sunshine 与 Moonlight 保存。

**Windows** 使用 Rust 1.89 或更新版本及 Visual Studio C++ 构建工具。单独构建主程序用 `cargo build --locked --release -p flashlink-host`。制作内含引擎的发行文件时，准备官方 `v2026.914.233613` 的 `Sunshine-Windows-AMD64-lite.zip` 及该版本的 LICENSE，并运行下列命令。打包脚本会比对引擎 SHA-256，并生成单文件 EXE 和便携 ZIP。[Sunshine 对应发行版](https://github.com/SunshineStream/Sunshine/releases/tag/v2026.914.233613)

```powershell
python packaging/windows/bundle.py --exe target/release/flashlink-host.exe --sunshine-zip Sunshine-Windows-AMD64-lite.zip --sunshine-license Sunshine-LICENSE
```

**Android** 使用 Flutter 3.38.9、JDK 17、Python 3、Git 和 Rust。设置 `ANDROID_HOME`、`JAVA_HOME`，在 Android SDK 中准备 Platform 36、Build Tools 35.0.0 与 36.0.0、NDK 28.2.13676358 和 CMake 3.22.1。

```sh
rustup target add aarch64-linux-android
python3 scripts/android_demo.py
```

默认 APK 为 `build/android-demo/flashlink-arm64-debug.apk`，应用 ID 为 `dev.flashlink.demo.debug`。本次版本为 `0.3.0-demo`（316），可以在 Android 系统设置的应用信息中核对。运行 `cargo test --locked --workspace` 测试 Rust。在 `apps/flashlink` 运行 `flutter analyze` 和 `flutter test`，可用 `--dart-define=FLASHLINK_TEST_CLIENT_LIBRARY=<本机动态库绝对路径>` 同时测试 Rust ABI。

Android 采用 Moonlight Android `v12.1` 和其固定的 `moonlight-common-c` 提交 `8af4562`。对应集成源码、修改说明与许可随试用文件提供。[Android 集成源码](dist/FlashLink-Android-source.zip) · [Moonlight 对应版本](https://github.com/moonlight-stream/moonlight-android/tree/1b1b100e63e8ffcf1496e324ff7b57ee9f2db2a9) · [许可说明](packaging/android/NOTICE.txt)


## 0.3.0-demo 验证记录

使用最终 APK、Android 14 arm64 模拟器和隔离 Linux Sunshine 桌面，短时断网从媒体回调报告中断到恢复画面约 7 秒，持续断网在检测后约 15 秒结束重试。客户端使用已保存的配对证书与主机 Rust 接收端完成了双向 TLS 心跳验证。输入测试在主机收到了左右中键、滚轮、相对移动和回车、空格的按下与释放。当前脚本会将每次实测数据写入本地 `testdata/`。[新版工具面板](dist/FlashLink-Android-stream.png) · [测试方法](docs/usage.md)

## 第一轮串流基线（0.2.0-demo）

在 Android 14 arm64 模拟器上实测了手机和平板布局、自动发现、首次请求、拒绝、允许、设备消失，以及保留身份后的直接连接。还将 Android 应用接到了隔离 Linux 虚拟机中的真实 Sunshine：两端确认码一致，允许后直接进入 Desktop，模拟器收到了虚拟桌面的实时时钟画面。覆盖安装最终 APK 后，第二次连接跳过配对，安卓收到持续变化的时钟画面。点按安卓屏幕后，主机的虚拟鼠标记录到左键按下和释放。此次主机使用 X11 和软件 H.264 编码，画面为 1280×720。服务端记录了加密视频和 48 kHz 双声道 Opus；安卓音频管线已启动，有一次 AudioTrack 缓冲不足并重启，模拟器关闭了声音输出。Windows 已交叉编译并检查打包，实体 Windows 的 GPU 编码、系统授权弹窗、实听效果及 1440p/60、120 FPS 仍需真机验收。
