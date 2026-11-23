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
  SessionPhase phase = SessionPhase.idle;
  String? feedback;
  String? activeDevice;
  String? discoveryError;
  bool scanning = false;
  bool scanned = false;
  bool _disposed = false;

  List<NearbyDevice> get nearby =>
      _known.values.where((d) => _visible.contains(d.id)).toList();
  List<NearbyDevice> get unavailable =>
      _known.values.where((d) => !_visible.contains(d.id)).toList();

  Future<void> discover() async {
    if (_disposed || scanning) return;
    scanning = true;
    _refresh();
    try {
      final devices = await _player.discover();
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
      scanning = false;
      scanned = true;
      _refresh();
    }
  }

  Future<void> openSettings() async {
    if (phase != SessionPhase.idle) return;
    try {
      await _player.openSettings();
    } catch (_) {
      feedback = '播放设置未能打开，请重试。';
      _refresh();
    }
  }

  Future<void> connect(NearbyDevice device) async {
    if (phase != SessionPhase.idle || !_visible.contains(device.id)) return;
    final target = _known[device.id]!;
    final SessionCore core;
    final int attemptId;
    feedback = null;
    try {
      core = _core ??= _loadCore();
      attemptId = core.begin(target.host);
    } catch (_) {
      feedback = '连接组件不可用，请重新安装 FlashLink。';
      _refresh();
      return;
    }
    if (attemptId <= 0) {
      feedback = attemptId == -2 ? '已有连接正在进行，请先返回设备页。' : '暂时无法连接这台电脑，请刷新设备列表。';
      _refresh();
      return;
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
      feedback = error.message?.trim().isNotEmpty == true
          ? error.message
          : '连接未完成，请重试。';
    } catch (_) {
      core.event(attemptId, PlayerEvent.launchFailed);
      feedback = '连接未能开始，请重试。';
    } finally {
      phase = core.phase;
      activeDevice = null;
      _refresh();
    }
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
