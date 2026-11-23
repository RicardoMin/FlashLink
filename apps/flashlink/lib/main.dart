import 'dart:async';

import 'package:flutter/material.dart';

import 'connection.dart';
import 'player.dart';
import 'session.dart';

void main() => runApp(const FlashLinkApp());

const _ink = Color(0xff15312c);
const _muted = Color(0xff526760);
const _green = Color(0xff176d58);
const _mint = Color(0xffa7f2cc);
const _paper = Color(0xfff5f7f3);
const _line = Color(0xffd7dfd7);

class FlashLinkApp extends StatelessWidget {
  const FlashLinkApp({super.key, this.controller});
  final ConnectionController? controller;

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'FlashLink',
        debugShowCheckedModeBanner: false,
        theme: ThemeData(
          useMaterial3: true,
          colorScheme:
              ColorScheme.fromSeed(seedColor: _green).copyWith(primary: _green),
          scaffoldBackgroundColor: _paper,
          dividerColor: _line,
        ),
        home: _DevicePage(controller: controller),
      );
}

class _DevicePage extends StatefulWidget {
  const _DevicePage({this.controller});
  final ConnectionController? controller;

  @override
  State<_DevicePage> createState() => _DevicePageState();
}

class _DevicePageState extends State<_DevicePage> with WidgetsBindingObserver {
  late final controller = widget.controller ??
      ConnectionController(NativeSessionCore.new, AndroidPlayer());
  Timer? _poll;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _startDiscovery();
  }

  void _startDiscovery() {
    _poll?.cancel();
    unawaited(controller.discover());
    _poll = Timer.periodic(
        const Duration(seconds: 10), (_) => controller.discover());
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) {
      _startDiscovery();
    } else {
      _poll?.cancel();
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _poll?.cancel();
    if (widget.controller == null) controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 1100),
            child: LayoutBuilder(builder: (context, box) {
              final wide = box.maxWidth >= 780;
              return ListenableBuilder(
                  listenable: controller,
                  builder: (context, _) {
                    final busy = controller.phase != SessionPhase.idle;
                    final nearby = controller.nearby;
                    final missing = controller.unavailable;
                    final firstScan = !controller.scanned;
                    final devices = Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          if (nearby.isEmpty)
                            _DiscoveryState(
                                scanning: firstScan,
                                failure: controller.discoveryError)
                          else
                            for (final device in nearby)
                              Padding(
                                padding: const EdgeInsets.only(bottom: 12),
                                child: _DeviceCard(
                                  device: device,
                                  opening: device.id == controller.activeDevice,
                                  onTap: busy
                                      ? null
                                      : () => controller.connect(device),
                                ),
                              ),
                          if (missing.isNotEmpty) ...[
                            const SizedBox(height: 20),
                            const Text('此前发现',
                                style: TextStyle(
                                    fontSize: 16,
                                    fontWeight: FontWeight.w700,
                                    color: _ink)),
                            const SizedBox(height: 10),
                            for (final device in missing)
                              ListTile(
                                contentPadding:
                                    const EdgeInsets.symmetric(horizontal: 8),
                                leading: const Icon(
                                    Icons.desktop_windows_outlined,
                                    color: _muted),
                                title: Text(device.name,
                                    maxLines: 2,
                                    overflow: TextOverflow.ellipsis),
                                trailing: Text(
                                    controller.discoveryError == null
                                        ? '本次未发现'
                                        : '状态待确认',
                                    style: const TextStyle(color: _muted)),
                              ),
                          ],
                          if (controller.feedback != null) ...[
                            const SizedBox(height: 12),
                            Semantics(
                                liveRegion: true,
                                child: Text(controller.feedback!,
                                    style: TextStyle(
                                        color: Theme.of(context)
                                            .colorScheme
                                            .error))),
                          ],
                        ]);
                    return RefreshIndicator(
                      onRefresh: controller.discover,
                      child: ListView(
                        physics: const AlwaysScrollableScrollPhysics(),
                        padding: EdgeInsets.fromLTRB(
                            wide ? 48 : 20, wide ? 28 : 14, wide ? 48 : 20, 32),
                        children: [
                          Row(children: [
                            Container(
                              height: 36,
                              width: 36,
                              decoration: BoxDecoration(
                                  color: _ink,
                                  borderRadius: BorderRadius.circular(11)),
                              child: const Icon(Icons.bolt_rounded,
                                  color: _mint, size: 29),
                            ),
                            const SizedBox(width: 9),
                            const Expanded(
                                child: Text('FlashLink',
                                    style: TextStyle(
                                        color: _ink,
                                        fontSize: 22,
                                        fontWeight: FontWeight.w800,
                                        letterSpacing: -.8))),
                            TextButton.icon(
                              icon: const Icon(Icons.tune_rounded, size: 19),
                              onPressed: busy ? null : controller.openSettings,
                              label: const Text('播放设置'),
                            ),
                          ]),
                          const SizedBox(height: 16),
                          const Divider(height: 1),
                          SizedBox(height: wide ? 40 : 26),
                          Row(
                              crossAxisAlignment: CrossAxisAlignment.center,
                              children: [
                                const Expanded(
                                    child: Text('附近的电脑',
                                        style: TextStyle(
                                            fontSize: 32,
                                            fontWeight: FontWeight.w800,
                                            height: 1.2,
                                            color: _ink))),
                                IconButton.filledTonal(
                                  style: IconButton.styleFrom(
                                      backgroundColor: const Color(0xffe3eee3),
                                      foregroundColor: _green),
                                  onPressed: busy || controller.scanning
                                      ? null
                                      : controller.discover,
                                  tooltip: '刷新设备',
                                  icon: const Icon(Icons.refresh_rounded),
                                ),
                              ]),
                          const SizedBox(height: 8),
                          Text(
                              nearby.isNotEmpty
                                  ? '选择一台电脑，连接它的 Windows 桌面。'
                                  : '自动查找同一局域网中的 FlashLink 电脑。',
                              style: const TextStyle(
                                  fontSize: 15, height: 1.45, color: _muted)),
                          const SizedBox(height: 23),
                          if (wide)
                            Row(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Expanded(flex: 5, child: devices),
                                  const SizedBox(width: 22),
                                  const Expanded(
                                      flex: 3, child: _ConnectionGuide()),
                                ])
                          else ...[
                            devices,
                            const SizedBox(height: 16),
                            const _ConnectionGuide(),
                          ],
                        ],
                      ),
                    );
                  });
            }),
          ),
        ),
      ),
    );
  }
}

class _DeviceCard extends StatelessWidget {
  const _DeviceCard(
      {required this.device, required this.opening, required this.onTap});
  final NearbyDevice device;
  final bool opening;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    const light = Color(0xffd8eae0);
    return Semantics(
      container: true,
      button: true,
      enabled: onTap != null,
      label: '连接 ${device.name}',
      child: Material(
        color: _ink,
        borderRadius: BorderRadius.circular(22),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.all(23),
            child:
                Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                Container(
                  padding: const EdgeInsets.all(10),
                  decoration: BoxDecoration(
                      color: const Color(0xff2a4b41),
                      borderRadius: BorderRadius.circular(12)),
                  child: const Icon(Icons.desktop_windows_rounded,
                      color: _mint, size: 28),
                ),
                const Spacer(),
                const Icon(Icons.circle, size: 8, color: _mint),
                const SizedBox(width: 6),
                const Text('已发现', style: TextStyle(color: light)),
              ]),
              const SizedBox(height: 23),
              Text(device.name,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                      fontSize: 25,
                      fontWeight: FontWeight.w700,
                      color: Colors.white,
                      height: 1.18)),
              const SizedBox(height: 6),
              const Text('Windows 电脑', style: TextStyle(color: light)),
              const SizedBox(height: 18),
              const Divider(height: 1, color: Color(0xff436158)),
              const SizedBox(height: 14),
              Row(children: [
                Expanded(
                    child: Text(opening ? '正在打开连接…' : '连接电脑',
                        style: const TextStyle(
                            color: _mint,
                            fontSize: 15,
                            fontWeight: FontWeight.w700))),
                const Icon(Icons.arrow_forward_rounded, color: _mint, size: 21),
              ]),
            ]),
          ),
        ),
      ),
    );
  }
}

class _DiscoveryState extends StatelessWidget {
  const _DiscoveryState({required this.scanning, required this.failure});
  final bool scanning;
  final String? failure;

  @override
  Widget build(BuildContext context) {
    final title = scanning
        ? '正在查找电脑…'
        : failure == null
            ? '暂未发现电脑'
            : '无法查找电脑';
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 32, horizontal: 22),
      decoration: BoxDecoration(
          color: const Color(0xffe7eee6),
          borderRadius: BorderRadius.circular(22)),
      child: Column(children: [
        Container(
          height: 58,
          width: 58,
          decoration: BoxDecoration(
              color: Colors.white, borderRadius: BorderRadius.circular(18)),
          child: Icon(
              failure == null ? Icons.devices_rounded : Icons.wifi_off_rounded,
              size: 31,
              color: _green),
        ),
        const SizedBox(height: 15),
        Text(title,
            textAlign: TextAlign.center,
            style: const TextStyle(
                color: _ink, fontSize: 20, fontWeight: FontWeight.w700)),
        const SizedBox(height: 7),
        Text(
            failure ??
                (scanning
                    ? '正在等待局域网中的电脑回应。'
                    : '请在电脑上打开 FlashLink，并确认两台设备连到同一个网络。'),
            textAlign: TextAlign.center,
            style: const TextStyle(color: _muted, height: 1.5)),
      ]),
    );
  }
}

class _ConnectionGuide extends StatelessWidget {
  const _ConnectionGuide();

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(21),
      decoration: BoxDecoration(
          color: Colors.white,
          border: Border.all(color: _line),
          borderRadius: BorderRadius.circular(20)),
      child:
          const Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text('首次连接',
            style: TextStyle(
                fontSize: 18, fontWeight: FontWeight.w700, color: _ink)),
        SizedBox(height: 17),
        _GuideItem(
            icon: Icons.power_settings_new_rounded,
            heading: '在电脑上打开 FlashLink',
            detail: '确保 Windows 已登录并解锁。'),
        SizedBox(height: 16),
        _GuideItem(
            icon: Icons.pin_outlined,
            heading: '在电脑上确认配对',
            detail: '核对两端显示的完整配对码，确认一致后在电脑上点「允许」。'),
      ]),
    );
  }
}

class _GuideItem extends StatelessWidget {
  const _GuideItem(
      {required this.icon, required this.heading, required this.detail});
  final IconData icon;
  final String heading;
  final String detail;

  @override
  Widget build(BuildContext context) =>
      Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Icon(icon, size: 20, color: _green),
        const SizedBox(width: 10),
        Expanded(
            child:
                Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Text(heading,
              style: const TextStyle(
                  fontWeight: FontWeight.w600, height: 1.4, color: _ink)),
          const SizedBox(height: 3),
          Text(detail,
              style: const TextStyle(height: 1.5, fontSize: 13, color: _muted)),
        ])),
      ]);
}
