import 'dart:async';

import 'package:flashlink/player.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('dev.flashlink/player');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  test('decodes discovered device identities and opens native settings',
      () async {
    final calls = <String>[];
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call.method);
      expect(call.arguments, isNull);
      return call.method == 'discover'
          ? [
              {
                'id':
                    'c89278878938771fba8e6c3e6475a79500099056e995880455fcce81f3585767',
                'name': 'Game PC',
                'host': '192.168.0.8:47989'
              }
            ]
          : null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    final player = AndroidPlayer(channel);
    final devices = await player.discover();
    expect(devices.single.name, 'Game PC');
    expect(devices.single.host, '192.168.0.8:47989');
    await player.openSettings();
    expect(calls, ['discover', 'settings']);
  });

  test(
      'opens once for the current native attempt and waits for activity result',
      () async {
    final exit = Completer<void>();
    final received = Completer<MethodCall>();
    messenger.setMockMethodCallHandler(channel, (call) async {
      received.complete(call);
      await exit.future;
      return null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    var opened = 0;
    var returned = false;
    final player = AndroidPlayer(channel);
    final launch = player
        .launch(
            'game-pc', 'Game PC', 'certificate-fingerprint', 8, () => opened++)
        .then((_) => returned = true);
    final call = await received.future;
    expect(call.method, 'launch');
    expect(call.arguments, {
      'host': 'game-pc',
      'name': 'Game PC',
      'id': 'certificate-fingerprint',
      'attemptId': 8
    });
    Future<void> notify(int id) async {
      final response = Completer<void>();
      await messenger.handlePlatformMessage(
        channel.name,
        channel.codec.encodeMethodCall(MethodCall('opened', {'attemptId': id})),
        (_) => response.complete(),
      );
      await response.future;
    }

    await notify(7);
    expect(opened, 0);
    await notify(8);
    expect(opened, 1);
    expect(returned, isFalse);
    exit.complete();
    await launch;
    expect(returned, isTrue);
  });
}
