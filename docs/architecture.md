# FlashLink 当前架构

FlashLink `0.3.0-demo` 在同一 IPv4 局域网内，把已登录、已解锁的 Windows 10/11 x64 桌面串流到 Android arm64 手机或平板。Windows 应用管理 Sunshine 的生命周期和首次授权，Android 应用发现主机并启动内嵌的 Moonlight 播放器。两端保存配对身份，后续连接可以直接进入桌面。

## 运行时与代码入口

```mermaid
flowchart LR
  subgraph Android["Android · FlashLink APK"]
    UI["Flutter 设备页"]
    Core["Rust 连接状态"]
    Native["Java 发现与配对"]
    Player["Moonlight 原生播放器"]
    UI <-->|Dart FFI| Core
    UI <-->|MethodChannel| Native
    Native -->|启动 Desktop| Player
  end
  subgraph Windows["Windows · 当前用户会话"]
    Host["Rust 主机窗口与授权"]
    Sun["Sunshine 子进程"]
    Host -->|启动、监督、本机管理 API| Sun
  end
  Native <-->|局域网发现与首次授权| Host
  Native <-->|配对握手与 Desktop 查询| Sun
  Sun -->|加密视频与音频| Player
  Player -->|触控映射的鼠标等输入| Sun
```

| 模块 | 当前职责 | 源码入口 |
| --- | --- | --- |
| Windows 主机 | Rust 与 Win32 实现主窗口和允许／拒绝弹窗。就绪检查涵盖引擎子进程、GameStream 服务和仅本机可用的 HTTPS 管理接口。Job Object 随主机退出清理 Sunshine。 | [主机编排](../crates/host-windows/src/service.rs)、[本机管理](../crates/host-windows/src/management.rs) |
| Windows 媒体引擎 | Sunshine 采集 Windows 桌面及系统声音，选择可用编码器，发送加密串流并执行回传输入。专用配置只发布名为 `Desktop` 的桌面入口。 | [引擎配置](../crates/host-windows/src/profile.rs) |
| Android 界面与状态 | Flutter 展示附近设备、处理刷新和播放设置。Dart 调用 Rust 的 C ABI 校验目标并分配连接编号；同时只允许一个活动的播放器打开请求，重复点击和旧回调按编号处理。 | [Flutter 控制器](../apps/flashlink/lib/connection.dart)、[Rust C ABI](../crates/client/include/flashlink_client.h) |
| Android 原生串流 | Java 适配器通过平台通道接收设备选择，负责发现、初次确认、查找已配对主机及启动桌面。Moonlight 完成协议握手、原生视频解码、声音播放和输入回传。媒体帧直接进入原生管线。 | [Android 宿主](../packaging/android/FlashLinkActivity.java)、[会话入口](../packaging/android/FlashLinkSessionActivity.java)、[Moonlight 集成补丁](../packaging/android/moonlight-v12.1.patch) |

## 从发现到直连

1. **发现。** Android 在局域网发送带随机值的 UDP 查询。已就绪的 Windows 主机在端口 `47980` 回送该随机值、电脑名、串流基准端口和主机证书的 SHA-256 指纹。安卓按证书指纹合并设备，使用应答报文的来源 IP 建立连接。串流基准端口默认为 `47989`。[安卓发现](../packaging/android/FlashLinkDiscovery.java) · [主机发现与授权队列](../crates/host-windows/src/lan.rs)
2. **首次授权。** Android 连接基准端口减 8 的 TLS 服务，核对服务证书与发现指纹，再发送一次性请求编号、临时 PIN 和设备名称。同时，Moonlight 以同一编号发起配对。Windows 匹配请求编号和来源地址，在两端展示由主机指纹前缀与临时 PIN 组成的确认码。Windows 用户核对并允许后，主机程序向 Sunshine 本机管理接口批准对应请求；拒绝或超时会结束本次授权。[安卓授权通道](../packaging/android/FlashLinkConsent.java) · [Windows 授权处理](../crates/host-windows/src/service.rs)
3. **保存身份。** Moonlight 完成握手并核对主机证书后，Android 将主机身份写入其设备库；Sunshine 保存已授权的客户端身份。Windows 默认将主机证书、配对记录和日志保存在当前用户的 `%LOCALAPPDATA%\FlashLink\DemoHost`。[客户端身份处理](../packaging/android/FlashLinkSessionActivity.java) · [Windows 配置目录](../crates/host-windows/src/profile.rs)
4. **再次连接。** Android 从新的发现应答取得最新地址，用保存的证书验证主机和配对状态，通过已认证连接查询 `Desktop` 的实际应用 ID，再启动原生播放器。电脑更换局域网 IP 后仍可按证书识别；主机证书变化或配对已失效时，需要重新确认。

## 播放、重连与隐私

Flutter 呈现手机设备列表及平板的列表／详情双栏。串流打开后由 Android 原生播放器管理覆盖在画面上的鼠标和工具面板。重连逻辑监听实际媒体回调，使用单调时钟、退避间隔与 15 秒期限；只使用已配对身份，主动断开不触发重连。播放器将分时段的接收帧、提交至 Surface 的帧以及丢帧写入诊断计数。[原生播放器](../packaging/android/FlashLinkGame.java) · [重连策略](../packaging/android/FlashLinkReconnect.java)

Windows 主进程提供系统托盘、逐设备撤销和报告导出。启用隐私后，独立 Rust 守护进程保存显示恢复信息，调用 Windows 显示配置 API，仅启用已识别的 MttVDD 虚拟输出并检查活动显示路径。安卓在原生播放器运行时通过双向 TLS 发送心跳；主机按 Sunshine 中已授权的客户端证书验证身份。心跳中断约 5 秒后进入 15 秒宽限期；期限结束后，守护进程停止串流、请求 Windows 锁定、确认会话锁定，再恢复显示。主机确认恢复完成且本机已解锁后重新开启引擎与发现，沿用原配对记录。诊断文件记录这些状态的先后顺序。[显示拓扑](../crates/host-windows/src/privacy_display.rs) · [隐私守护进程](../crates/host-windows/src/privacy.rs) · [安卓心跳](../packaging/android/FlashLinkPrivacyLease.java)

## 构建与验证范围

Windows 打包工具把 Rust 主程序与固定版本 Sunshine `v2026.914.233613` 生成离线单文件 EXE 和便携 ZIP。Android 构建脚本从固定提交的 Moonlight Android `v12.1` 应用补丁，将 Flutter AAR、Rust 动态库和 Moonlight 原生库打进同一个 APK。[Windows 打包](../packaging/windows/bundle.py) · [Android 构建](../scripts/android_demo.py)

Android 模拟器与隔离 Linux 主机上的真实 Sunshine 已验证首次配对、再次直连、720p 软件编码桌面画面及点按回传。Windows x64 主程序已交叉编译并检查打包。Windows 实体机器的 GPU 编码、实际听感以及 1440p/60 FPS 和 120 FPS 表现仍待真机验证。Windows 隐私切屏、多屏热插拔、异常退出时是否短暂露出桌面，以及锁屏后能否恢复原显示目标，均需实体显示器验收。
