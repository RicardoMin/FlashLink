import 'dart:async';

import 'package:flashlink/connection.dart';
import 'package:flashlink/main.dart';
import 'package:flashlink/player.dart';
import 'package:flashlink/session.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

const device = NearbyDevice(
    id: 'c89278878938771fba8e6c3e6475a79500099056e995880455fcce81f3585767',
    name: '书房电脑',
    host: '192.168.1.67:47989');

class FakeCore implements SessionCore {
  final events = <PlayerEvent>[];
  String? requested;
  @override
  SessionPhase phase = SessionPhase.idle;
  @override
  int begin(String host) {
    if (phase != SessionPhase.idle) return -2;
    requested = host;
    phase = SessionPhase.launching;
    return 7;
  }

  @override
  String target(int attemptId) => 'canonical-host:47989';
  @override
  bool event(int attemptId, PlayerEvent event) {
    if (attemptId != 7 || phase == SessionPhase.idle) return false;
    events.add(event);
    phase = event == PlayerEvent.opened
        ? SessionPhase.playerOpen
        : SessionPhase.idle;
    return true;
  }
}

class FakePlayer implements Player {
  final exit = Completer<void>();
  final launched = <Object>[];
  late VoidCallback opened;
  int scans = 0;
  int settingsCount = 0;
  Object? fail;
  Completer<List<NearbyDevice>>? pendingScan;
  List<NearbyDevice> devices = [device];
  @override
  Future<void> openSettings() async => settingsCount++;

  @override
  Future<List<NearbyDevice>> discover() async {
    scans++;
    if (fail case final err?) throw err;
    if (pendingScan case final pending?) return pending.future;
    return devices;
  }

  @override
  Future<void> launch(String host, String name, String deviceId, int attemptId,
      VoidCallback onOpened) {
    launched.addAll([host, name, deviceId, attemptId]);
    opened = onOpened;
    return exit.future;
  }
}

void main() {
  test('overlapping refreshes wait for the same discovery result', () async {
    final scan = Completer<List<NearbyDevice>>();
    final player = FakePlayer()..pendingScan = scan;
    final controller = ConnectionController(FakeCore.new, player);
    addTearDown(controller.dispose);

    final automatic = controller.discover();
    var refreshed = false;
    final manual = controller.discover().then((_) => refreshed = true);
    await Future<void>.value();
    expect(refreshed, isFalse);
    expect(controller.scanning, isTrue);
    expect(player.scans, 1);
    scan.complete([device]);
    await manual;
    expect(controller.nearby, [device]);
    expect(controller.scanning, isFalse);
    await automatic;
  });

  test('lost device cannot launch and discovery failure does not imply offline',
      () async {
    final core = FakeCore();
    final player = FakePlayer();
    final controller = ConnectionController(() => core, player);
    addTearDown(controller.dispose);
    await controller.discover();
    expect(controller.nearby, [device]);
    player.devices = [];
    await controller.discover();
    expect(controller.unavailable, [device]);
    expect(controller.discoveryError, isNull);
    await controller.connect(device);
    expect(player.launched, isEmpty);
    player.fail = const FormatException();
    await controller.discover();
    expect(controller.discoveryError, isNotNull);
    expect(controller.unavailable, [device]);
    player
      ..fail = null
      ..devices = [device];
    await controller.discover();
    expect(controller.discoveryError, isNull);
    expect(controller.unavailable, isEmpty);
    expect(controller.nearby, [device]);
  });

  for (final width in [360.0, 1024.0]) {
    testWidgets('opens discovered device using pinned identity at width $width',
        (tester) async {
      tester.view.physicalSize = Size(width, 800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final core = FakeCore();
      final player = FakePlayer();
      final controller = ConnectionController(() => core, player);
      addTearDown(controller.dispose);
      await tester.pumpWidget(FlashLinkApp(controller: controller));
      await tester.pumpAndSettle();
      expect(player.scans, 1);
      expect(find.text('在线设备 1'), findsOneWidget);
      await tester.tap(find.text('书房电脑').first);
      await tester.pump();
      expect(core.requested, device.host);
      expect(
          player.launched, ['canonical-host:47989', device.name, device.id, 7]);
      await controller.connect(device);
      await controller.openSettings();
      expect(player.launched.length, 4);
      expect(player.settingsCount, 0);
      player.opened();
      expect(controller.phase, SessionPhase.playerOpen);
      player.exit.complete();
      await tester.pumpAndSettle();
      expect(core.events, [PlayerEvent.opened, PlayerEvent.returned]);
      expect(controller.phase, SessionPhase.idle);
      await tester.tap(find.text('设置'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('播放设置'));
      expect(player.settingsCount, 1);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('pairing errors are shown once without leaking into settings',
      (tester) async {
    final player = FakePlayer();
    final controller = ConnectionController(FakeCore.new, player);
    addTearDown(controller.dispose);
    await tester.pumpWidget(FlashLinkApp(controller: controller));
    await tester.pumpAndSettle();
    await tester.tap(find.text(device.name).first);
    player.exit
        .completeError(PlatformException(code: 'denied', message: '配对被拒绝'));
    await tester.pumpAndSettle();
    expect(find.widgetWithText(SnackBar, '配对被拒绝'), findsOneWidget);
    await tester.pump(const Duration(seconds: 5));
    await tester.pumpAndSettle();
    await tester.tap(find.text('设置'));
    await tester.pumpAndSettle();
    expect(find.text('配对被拒绝'), findsNothing);
    expect(find.text('播放设置'), findsOneWidget);
  });

  testWidgets('foreground refresh and manual refresh repair discovery failures',
      (tester) async {
    final player = FakePlayer()..fail = PlatformException(code: 'no_wifi');
    final controller = ConnectionController(FakeCore.new, player);
    addTearDown(controller.dispose);
    await tester.pumpWidget(FlashLinkApp(controller: controller));
    await tester.pumpAndSettle();
    expect(find.text('无法查找电脑'), findsOneWidget);
    player
      ..fail = null
      ..devices = [];
    await tester.tap(find.byTooltip('刷新设备'));
    await tester.pumpAndSettle();
    expect(find.text('暂未发现电脑'), findsOneWidget);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    player.devices = [device];
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpAndSettle();
    expect(find.text('书房电脑'), findsWidgets);
    expect(player.scans, 3);
    await tester.tap(find.text('连接帮助'));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 20));
    expect(player.scans, 3);
    await tester.tap(find.text('我的设备'));
    await tester.pumpAndSettle();
    expect(player.scans, 4);
  });
  testWidgets('device details follow a lost host and filter hides missing rows',
      (tester) async {
    tester.view.physicalSize = const Size(375, 760);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final player = FakePlayer();
    final controller = ConnectionController(FakeCore.new, player);
    addTearDown(controller.dispose);
    await tester.pumpWidget(FlashLinkApp(controller: controller));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('查看书房电脑详情'));
    await tester.pumpAndSettle();
    expect(find.text('进入桌面'), findsOneWidget);
    player.devices = [];
    await controller.discover();
    await tester.pumpAndSettle();
    await tester.tap(find.text('设备不可用'));
    expect(player.launched, isEmpty);
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('此前发现 1'), findsOneWidget);
    await tester.tap(find.byTooltip('筛选设备'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('仅在线设备'));
    await tester.pumpAndSettle();
    expect(find.text('此前发现 1'), findsNothing);
    expect(find.text('暂未发现电脑'), findsOneWidget);
    await tester.tap(find.text('首次连接'));
    await tester.pumpAndSettle();
    expect(find.text('核对配对码'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
