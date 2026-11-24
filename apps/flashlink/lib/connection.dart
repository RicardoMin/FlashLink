import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'player.dart';
import 'session.dart';

final class ConnectionController extends ChangeNotifier {
  ConnectionController(this._loadCore, this._player);

  final SessionCore Function() _loadCore;
  final Player _player;
  final _known = <String, NearbyDevice>{};
  Set<String> _visible = {};
  SessionCore? _core;
  Future<void>? _discovery;
  SessionPhase phase = SessionPhase.idle;
  String? activeDevice;
  String? discoveryError;
  bool get scanning => _discovery != null;
  bool scanned = false;
  bool _disposed = false;

  List<NearbyDevice> get nearby =>
      _known.values.where((d) => _visible.contains(d.id)).toList();
  List<NearbyDevice> get unavailable =>
      _known.values.where((d) => !_visible.contains(d.id)).toList();

  Future<void> discover() {
    if (_disposed) return Future.value();
    final pending = _discovery;
    if (pending != null) return pending;
    final request = _discovery = _scan();
    _refresh();
    return request;
  }

  Future<void> _scan() async {
    try {
      final devices = await Future.sync(_player.discover);
      if (_disposed) return;
      _visible = {for (final device in devices) device.id};
      for (final device in devices) {
        _known[device.id] = device;
      }
      discoveryError = null;
    } catch (_) {
      _visible = {};
      discoveryError = '设备发现暂时不可用，请检查 Wi-Fi 后重新查找。';
    } finally {
      _discovery = null;
      scanned = true;
      _refresh();
    }
  }

  Future<String?> openSettings() async {
    if (_disposed || phase != SessionPhase.idle) return null;
    try {
      await _player.openSettings();
    } catch (_) {
      return '播放设置未能打开，请重试。';
    }
    return null;
  }

  Future<String?> connect(NearbyDevice device) async {
    if (_disposed ||
        phase != SessionPhase.idle ||
        !_visible.contains(device.id)) {
      return null;
    }
    final target = _known[device.id]!;
    final SessionCore core;
    final int attemptId;
    try {
      core = _core ??= _loadCore();
      attemptId = core.begin(target.host);
    } catch (_) {
      return '连接组件不可用，请重新安装 FlashLink。';
    }
    if (attemptId <= 0) {
      return attemptId == -2 ? '已有连接正在进行，请先返回设备页。' : '暂时无法连接这台电脑，请刷新设备列表。';
    }
    activeDevice = target.id;
    phase = core.phase;
    _refresh();
    try {
      await _player.launch(
          core.target(attemptId), target.name, target.id, attemptId, () {
        if (core.event(attemptId, PlayerEvent.opened)) {
          phase = core.phase;
          _refresh();
        }
      });
      core.event(attemptId, PlayerEvent.returned);
    } on PlatformException catch (error) {
      core.event(attemptId, PlayerEvent.launchFailed);
      return error.message?.trim().isNotEmpty == true
          ? error.message
          : '连接未完成，请重试。';
    } catch (_) {
      core.event(attemptId, PlayerEvent.launchFailed);
      return '连接未能开始，请重试。';
    } finally {
      phase = core.phase;
      activeDevice = null;
      _refresh();
    }
    return null;
  }

  void _refresh() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
