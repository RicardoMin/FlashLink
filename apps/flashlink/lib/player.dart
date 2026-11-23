import 'package:flutter/services.dart';

final class NearbyDevice {
  const NearbyDevice(
      {required this.id, required this.name, required this.host});
  final String id;
  final String name;
  final String host;

  factory NearbyDevice.fromMap(Map<dynamic, dynamic> row) {
    final id = row['id'];
    final name = row['name'];
    final host = row['host'];
    if (id is! String ||
        id.isEmpty ||
        name is! String ||
        name.trim().isEmpty ||
        host is! String ||
        host.isEmpty) {
      throw const FormatException('Invalid discovery entry');
    }
    return NearbyDevice(id: id, name: name.trim(), host: host);
  }
}

abstract interface class Player {
  Future<List<NearbyDevice>> discover();
  Future<void> launch(String host, String name, String deviceId, int attemptId,
      VoidCallback onOpened);
  Future<void> openSettings();
}

/// The native launch result completes when the embedded activity returns.
final class AndroidPlayer implements Player {
  AndroidPlayer([this.channel = const MethodChannel('dev.flashlink/player')]);
  final MethodChannel channel;

  @override
  Future<List<NearbyDevice>> discover() async {
    final rows = await channel.invokeListMethod<Object?>('discover');
    if (rows == null) throw const FormatException('Missing discovery result');
    return [for (final row in rows) NearbyDevice.fromMap(row as Map)];
  }

  @override
  Future<void> openSettings() => channel.invokeMethod<void>('settings');

  @override
  Future<void> launch(String host, String name, String deviceId, int attemptId,
      VoidCallback onOpened) async {
    channel.setMethodCallHandler((call) async {
      final arguments = call.arguments;
      if (call.method == 'opened' &&
          arguments is Map &&
          arguments['attemptId'] == attemptId) {
        onOpened();
      }
    });
    try {
      await channel.invokeMethod<void>('launch',
          {'host': host, 'name': name, 'id': deviceId, 'attemptId': attemptId});
    } finally {
      channel.setMethodCallHandler(null);
    }
  }
}
