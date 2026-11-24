import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'connection.dart';
import 'device_cover.dart';
import 'player.dart';
import 'session.dart';

void main() => runApp(const FlashLinkApp());

const _ink = Color(0xff191e25);
const _muted = Color(0xff5d6773);
const _blue = Color(0xff327bfa);
const _paper = Color(0xfff1f5f8);
const _tint = Color(0xffe5ebef);
const _heading =
    TextStyle(color: _ink, fontSize: 25, fontWeight: FontWeight.w700);

class FlashLinkApp extends StatelessWidget {
  const FlashLinkApp({super.key, this.controller});
  final ConnectionController? controller;

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'FlashLink',
        debugShowCheckedModeBanner: false,
        theme: ThemeData(
          useMaterial3: true,
          colorScheme: ColorScheme.fromSeed(seedColor: _blue)
              .copyWith(primary: _blue, surface: Colors.white),
          scaffoldBackgroundColor: _paper,
          splashFactory: InkRipple.splashFactory,
        ),
        home: _Home(controller: controller),
      );
}

enum _Tab { devices, help, settings }

enum _Filter { all, online }

class _Home extends StatefulWidget {
  const _Home({this.controller});
  final ConnectionController? controller;
  @override
  State<_Home> createState() => _HomeState();
}

class _HomeState extends State<_Home> with WidgetsBindingObserver {
  late final controller = widget.controller ??
      ConnectionController(NativeSessionCore.new, AndroidPlayer());
  Timer? _poll;
  var _tab = _Tab.devices;
  var _filter = _Filter.all;
  bool _showOnline = true, _showMissing = true;
  String? _selected;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _updateDiscovery();
  }

  void _updateDiscovery() {
    _poll?.cancel();
    final state = WidgetsBinding.instance.lifecycleState;
    if (_tab != _Tab.devices ||
        (state != null && state != AppLifecycleState.resumed)) {
      return;
    }
    unawaited(controller.discover());
    _poll = Timer.periodic(
        const Duration(seconds: 10), (_) => controller.discover());
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) =>
      _updateDiscovery();

  void _selectTab(_Tab tab) {
    if (tab == _tab) return;
    setState(() => _tab = tab);
    _updateDiscovery();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _poll?.cancel();
    if (widget.controller == null) controller.dispose();
    super.dispose();
  }

  Future<void> _showResult(Future<String?> result) async {
    final message = await result;
    if (message == null || !mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message)));
  }

  void _openDetail(NearbyDevice device, bool wide) {
    setState(() => _selected = device.id);
    if (wide) return;
    Navigator.of(context).push(MaterialPageRoute<void>(
        builder: (context) => Scaffold(
              appBar:
                  AppBar(title: const Text('设备详情'), backgroundColor: _paper),
              body: SafeArea(
                  child: ListenableBuilder(
                      listenable: controller,
                      builder: (context, _) => ListView(
                          padding: const EdgeInsets.all(16),
                          children: [_detail(device)]))),
            )));
  }

  Widget _detail(NearbyDevice device) {
    final online =
        controller.nearby.where((d) => d.id == device.id).firstOrNull;
    final opening = controller.activeDevice == device.id;
    final busy = controller.phase != SessionPhase.idle;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Row(children: [
        _Status(
            online: online != null,
            uncertain: controller.discoveryError != null),
        const SizedBox(width: 8),
        Expanded(
            child: Text(online?.name ?? device.name,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: _heading.copyWith(fontSize: 22))),
      ]),
      const SizedBox(height: 16),
      Material(
          color: Colors.white,
          borderRadius: BorderRadius.circular(12),
          clipBehavior: Clip.antiAlias,
          child: Column(children: [
            AspectRatio(
                aspectRatio: 16 / 9,
                child: Stack(fit: StackFit.expand, children: [
                  DeviceCover(available: online != null),
                  const DecoratedBox(
                      decoration: BoxDecoration(
                          gradient: LinearGradient(
                              begin: Alignment.center,
                              end: Alignment.bottomCenter,
                              colors: [
                        Colors.transparent,
                        Color(0xaa0b2451)
                      ]))),
                  Material(
                      type: MaterialType.transparency,
                      child: InkWell(
                          onTap: busy || online == null
                              ? null
                              : () => _showResult(controller.connect(online)),
                          child: Align(
                              alignment: const Alignment(0, .8),
                              child: Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    Text(
                                        opening
                                            ? '正在连接…'
                                            : online != null
                                                ? '进入桌面'
                                                : '设备不可用',
                                        style: const TextStyle(
                                            color: Colors.white,
                                            fontSize: 18,
                                            fontWeight: FontWeight.w600)),
                                    if (online != null)
                                      const Icon(Icons.chevron_right,
                                          color: Colors.white),
                                  ])))),
                ])),
            Row(children: [
              Expanded(
                  child: TextButton.icon(
                      onPressed: busy
                          ? null
                          : () => _showResult(controller.openSettings()),
                      icon: const Icon(Icons.tune_rounded),
                      label: const Text('播放设置'))),
              const SizedBox(height: 18, child: VerticalDivider(width: 1)),
              Expanded(
                  child: TextButton.icon(
                      onPressed:
                          controller.scanning ? null : controller.discover,
                      icon: const Icon(Icons.refresh_rounded),
                      label: const Text('刷新状态'))),
            ]),
          ])),
    ]);
  }

  @override
  Widget build(BuildContext context) => AnnotatedRegion<SystemUiOverlayStyle>(
        value: const SystemUiOverlayStyle(
            statusBarColor: Colors.transparent,
            statusBarIconBrightness: Brightness.dark,
            systemNavigationBarColor: Color(0xfff9fbfc),
            systemNavigationBarIconBrightness: Brightness.dark),
        child: Scaffold(
          bottomNavigationBar: DecoratedBox(
              decoration: const BoxDecoration(
                  border: Border(top: BorderSide(color: Color(0x150c2440)))),
              child: BottomNavigationBar(
                elevation: 0,
                backgroundColor: const Color(0xfff9fbfc),
                selectedItemColor: _blue,
                unselectedItemColor: _muted,
                currentIndex: _tab.index,
                onTap: (i) => _selectTab(_Tab.values[i]),
                selectedFontSize: 12,
                unselectedFontSize: 12,
                items: const [
                  BottomNavigationBarItem(
                      icon: Icon(Icons.devices_outlined),
                      activeIcon: Icon(Icons.devices_rounded),
                      label: '我的设备'),
                  BottomNavigationBarItem(
                      icon: Icon(Icons.help_outline_rounded),
                      activeIcon: Icon(Icons.help_rounded),
                      label: '连接帮助'),
                  BottomNavigationBarItem(
                      icon: Icon(Icons.tune_rounded), label: '设置'),
                ],
              )),
          body: SafeArea(
              child: Align(
                  alignment: Alignment.topCenter,
                  child: ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 1140),
                      child: ListenableBuilder(
                          listenable: controller,
                          builder: (context, _) =>
                              LayoutBuilder(builder: _page))))),
        ),
      );
  Widget _page(BuildContext context, BoxConstraints box) {
    final wide = box.maxWidth >= 800;
    final padding =
        EdgeInsets.fromLTRB(wide ? 32 : 16, wide ? 28 : 18, wide ? 32 : 16, 24);
    if (_tab == _Tab.help) {
      return ListView(padding: padding, children: const [_Help()]);
    } else if (_tab == _Tab.settings) {
      return ListView(padding: padding, children: [
        const Text('设置', style: _heading),
        const SizedBox(height: 18),
        Material(
            color: Colors.white,
            borderRadius: BorderRadius.circular(12),
            clipBehavior: Clip.antiAlias,
            child: ListTile(
                leading: const Icon(Icons.tune_rounded, color: _blue),
                title: const Text('播放设置'),
                subtitle: const Text('分辨率、帧率、码率与性能统计'),
                trailing: const Icon(Icons.chevron_right),
                onTap: controller.phase == SessionPhase.idle
                    ? () => _showResult(controller.openSettings())
                    : null)),
      ]);
    }
    final nearby = controller.nearby;
    final missing = controller.unavailable;
    final filtered = [...nearby, if (_filter == _Filter.all) ...missing];
    final selected = filtered.where((d) => d.id == _selected).firstOrNull ??
        filtered.firstOrNull;
    final list =
        Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Row(children: [
        const Expanded(child: Text('可控设备', style: _heading)),
        PopupMenuButton<_Filter>(
            tooltip: '筛选设备',
            initialValue: _filter,
            onSelected: (v) => setState(() => _filter = v),
            itemBuilder: (_) => const [
                  PopupMenuItem(value: _Filter.all, child: Text('全部设备')),
                  PopupMenuItem(value: _Filter.online, child: Text('仅在线设备'))
                ],
            child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 7),
                child: DecoratedBox(
                    decoration: BoxDecoration(
                        color: _tint, borderRadius: BorderRadius.circular(20)),
                    child: Padding(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 10, vertical: 5),
                        child: Row(children: [
                          const Icon(Icons.devices_rounded, size: 15),
                          const SizedBox(width: 4),
                          Text(_filter == _Filter.all ? '全部设备' : '仅在线设备',
                              style: const TextStyle(fontSize: 13, color: _ink))
                        ]))))),
        IconButton(
            tooltip: '刷新设备',
            icon: const Icon(Icons.refresh_rounded, size: 21),
            onPressed: controller.scanning ? null : controller.discover),
      ]),
      _Group(
          label: '在线设备',
          count: nearby.length,
          expanded: _showOnline,
          onTap: () => setState(() => _showOnline = !_showOnline)),
      if (_showOnline) ...[
        if (nearby.isEmpty)
          _Empty(
              scanning: !controller.scanned || controller.scanning,
              failure: controller.discoveryError,
              onHelp: () => _selectTab(_Tab.help)),
        for (final device in nearby)
          _DeviceRow(
              device: device,
              online: true,
              opening: controller.activeDevice == device.id,
              onConnect: controller.phase == SessionPhase.idle
                  ? () => _showResult(controller.connect(device))
                  : null,
              onDetails: () => _openDetail(device, wide)),
      ],
      if (_filter == _Filter.all && missing.isNotEmpty) ...[
        const SizedBox(height: 12),
        _Group(
            label: '此前发现',
            count: missing.length,
            expanded: _showMissing,
            onTap: () => setState(() => _showMissing = !_showMissing)),
        if (_showMissing)
          for (final device in missing)
            _DeviceRow(
                device: device,
                online: false,
                uncertain: controller.discoveryError != null,
                onDetails: () => _openDetail(device, wide)),
      ],
    ]);
    return RefreshIndicator(
        onRefresh: controller.discover,
        child: ListView(
            physics: const AlwaysScrollableScrollPhysics(),
            padding: padding,
            children: [
              if (wide && selected != null)
                Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Expanded(flex: 5, child: list),
                  const SizedBox(width: 40),
                  Expanded(flex: 6, child: _detail(selected))
                ])
              else
                list,
            ]));
  }
}

class _Group extends StatelessWidget {
  const _Group(
      {required this.label,
      required this.count,
      required this.expanded,
      required this.onTap});
  final String label;
  final int count;
  final bool expanded;
  final VoidCallback onTap;
  @override
  Widget build(BuildContext context) => Align(
      alignment: Alignment.centerLeft,
      child: TextButton(
          onPressed: onTap,
          style: TextButton.styleFrom(
              foregroundColor: _muted,
              padding: const EdgeInsets.symmetric(horizontal: 4)),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            Text('$label $count'),
            const SizedBox(width: 3),
            Icon(expanded ? Icons.keyboard_arrow_up : Icons.keyboard_arrow_down,
                size: 17)
          ])));
}

class _Status extends StatelessWidget {
  const _Status({required this.online, this.uncertain = false});
  final bool online, uncertain;
  @override
  Widget build(BuildContext context) => DecoratedBox(
      decoration: BoxDecoration(
          color: const Color(0xda182433),
          borderRadius: BorderRadius.circular(20)),
      child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            Icon(Icons.circle,
                size: 7,
                color:
                    online ? const Color(0xff38da64) : const Color(0xffbfc6ce)),
            const SizedBox(width: 3),
            Text(
                online
                    ? '在线'
                    : uncertain
                        ? '待确认'
                        : '未发现',
                style: const TextStyle(
                    color: Colors.white, fontSize: 11, height: 1.2))
          ])));
}

class _DeviceRow extends StatelessWidget {
  const _DeviceRow(
      {required this.device,
      required this.online,
      required this.onDetails,
      this.opening = false,
      this.uncertain = false,
      this.onConnect});
  final NearbyDevice device;
  final bool online, opening, uncertain;
  final VoidCallback? onConnect;
  final VoidCallback onDetails;
  @override
  Widget build(BuildContext context) => Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Material(
          color: Colors.white,
          borderRadius: BorderRadius.circular(12),
          clipBehavior: Clip.antiAlias,
          child: Row(children: [
            Expanded(
                child: Semantics(
                    button: true,
                    label: online ? '连接 ${device.name}' : '查看 ${device.name}',
                    child: InkWell(
                        onTap: online ? onConnect : onDetails,
                        child: Row(children: [
                          SizedBox(
                              width: MediaQuery.sizeOf(context).width < 350
                                  ? 86
                                  : 116,
                              height: 76,
                              child: Stack(fit: StackFit.expand, children: [
                                DeviceCover(available: online),
                                Positioned(
                                    left: 5,
                                    top: 5,
                                    child: _Status(
                                        online: online, uncertain: uncertain))
                              ])),
                          const SizedBox(width: 14),
                          Expanded(
                              child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                Text(device.name,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: TextStyle(
                                        fontSize: 17,
                                        fontWeight: FontWeight.w500,
                                        color: online ? _ink : _muted)),
                                const SizedBox(height: 5),
                                Row(children: [
                                  const Icon(Icons.desktop_windows_outlined,
                                      size: 15, color: _blue),
                                  const SizedBox(width: 4),
                                  Expanded(
                                      child: Text(opening ? '正在连接…' : 'Windows',
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                          style: const TextStyle(
                                              fontSize: 12, color: _muted)))
                                ])
                              ])),
                        ])))),
            IconButton(
                onPressed: onDetails,
                tooltip: '查看${device.name}详情',
                icon: const Icon(Icons.chevron_right_rounded,
                    color: Color(0xff88939d))),
          ])));
}

class _Empty extends StatelessWidget {
  const _Empty(
      {required this.scanning, required this.failure, required this.onHelp});
  final bool scanning;
  final String? failure;
  final VoidCallback onHelp;
  @override
  Widget build(BuildContext context) => Card.filled(
      margin: EdgeInsets.zero,
      color: Colors.white,
      child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 22, vertical: 26),
          child: Column(children: [
            const Icon(Icons.devices_rounded, color: _blue, size: 42),
            const SizedBox(height: 12),
            Text(
                scanning
                    ? '正在查找电脑…'
                    : failure != null
                        ? '无法查找电脑'
                        : '暂未发现电脑',
                style: const TextStyle(
                    color: _ink, fontSize: 18, fontWeight: FontWeight.w600)),
            const SizedBox(height: 6),
            Text(failure ?? '在电脑上打开 FlashLink，并让两端连接同一局域网。',
                textAlign: TextAlign.center,
                style: const TextStyle(color: _muted, height: 1.5)),
            TextButton(onPressed: onHelp, child: const Text('首次连接')),
          ])));
}

class _Help extends StatelessWidget {
  const _Help();
  @override
  Widget build(BuildContext context) =>
      Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        const Text('连接帮助', style: _heading),
        const SizedBox(height: 18),
        Material(
            color: Colors.white,
            borderRadius: BorderRadius.circular(12),
            clipBehavior: Clip.antiAlias,
            child: const Column(children: [
              ListTile(
                  leading: Icon(Icons.desktop_windows_rounded, color: _blue),
                  title: Text('首次连接'),
                  subtitle: Text('在已解锁的 Windows 电脑上打开 FlashLink，等待主机显示已就绪。')),
              Divider(height: 1, indent: 56),
              ListTile(
                  leading: Icon(Icons.wifi_rounded, color: _blue),
                  title: Text('在局域网中发现电脑'),
                  subtitle: Text('手机和平板连接同一网络，电脑也可以使用网线。在“我的设备”下拉刷新，然后点设备名称。')),
              Divider(height: 1, indent: 56),
              ListTile(
                  leading: Icon(Icons.verified_user_outlined, color: _blue),
                  title: Text('核对配对码'),
                  subtitle: Text('首次连接时核对两端的完整确认码，在电脑上点“允许连接”。之后点同一设备即可进入桌面。')),
            ]))
      ]);
}
