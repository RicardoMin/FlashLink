import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

enum SessionPhase { idle, launching, playerOpen }

enum PlayerEvent {
  opened(1),
  returned(2),
  launchFailed(3);

  const PlayerEvent(this.code);
  final int code;
}

abstract interface class SessionCore {
  int begin(String host);
  String target(int attemptId);
  SessionPhase get phase;
  bool event(int attemptId, PlayerEvent event);
}

final class NativeSessionCore implements SessionCore {
  NativeSessionCore()
      : this.fromLibrary(DynamicLibrary.open('libflashlink_client.so'));

  NativeSessionCore.fromLibrary(DynamicLibrary library) {
    final version = library.lookupFunction<Uint32 Function(), int Function()>(
        'fl_client_abi_version');
    if (version() != 1) {
      throw StateError('Unsupported FlashLink session ABI');
    }
    _begin = library.lookupFunction<Int64 Function(Pointer<Uint8>, Size),
        int Function(Pointer<Uint8>, int)>('fl_client_begin');
    _phase = library
        .lookupFunction<Int32 Function(), int Function()>('fl_client_phase');
    _target = library.lookupFunction<
        Int32 Function(Int64, Pointer<Uint8>, Size),
        int Function(int, Pointer<Uint8>, int)>('fl_client_target');
    _event = library.lookupFunction<Int32 Function(Int64, Int32),
        int Function(int, int)>('fl_client_event');
  }

  late final int Function(Pointer<Uint8>, int) _begin;
  late final int Function() _phase;
  late final int Function(int, Pointer<Uint8>, int) _target;
  late final int Function(int, int) _event;

  @override
  int begin(String host) {
    final bytes = utf8.encode(host);
    final buffer = calloc<Uint8>(bytes.isEmpty ? 1 : bytes.length);
    try {
      buffer.asTypedList(bytes.length).setAll(0, bytes);
      return _begin(buffer, bytes.length);
    } finally {
      calloc.free(buffer);
    }
  }

  @override
  String target(int attemptId) {
    const capacity = 260;
    final output = calloc<Uint8>(capacity);
    try {
      final length = _target(attemptId, output, capacity);
      if (length <= 0 || length > capacity) {
        throw StateError('No active stream target');
      }
      return utf8.decode(output.asTypedList(length));
    } finally {
      calloc.free(output);
    }
  }

  @override
  SessionPhase get phase => switch (_phase()) {
        0 => SessionPhase.idle,
        1 => SessionPhase.launching,
        2 => SessionPhase.playerOpen,
        _ => throw StateError('Unexpected Rust session phase'),
      };

  @override
  bool event(int attemptId, PlayerEvent event) =>
      _event(attemptId, event.code) == 1;
}
