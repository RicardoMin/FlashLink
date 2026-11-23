import 'dart:ffi';

import 'package:flashlink/session.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const library = String.fromEnvironment('FLASHLINK_TEST_CLIENT_LIBRARY');
  test('real Rust ABI normalizes the target and rejects stale callbacks', () {
    final core = NativeSessionCore.fromLibrary(DynamicLibrary.open(library));
    expect(core.begin('https://game-pc'), -1);
    final first = core.begin('Game-PC.LOCAL.');
    expect(first, isPositive);
    expect(core.phase, SessionPhase.launching);
    expect(core.target(first), 'game-pc.local');
    expect(core.begin('second-pc'), -2);
    expect(core.event(first, PlayerEvent.opened), isTrue);
    expect(core.phase, SessionPhase.playerOpen);
    expect(core.event(first, PlayerEvent.returned), isTrue);
    final second = core.begin('[fd12::1]');
    expect(second, greaterThan(first));
    expect(core.event(first, PlayerEvent.returned), isFalse);
    expect(core.phase, SessionPhase.launching);
    expect(core.event(second, PlayerEvent.launchFailed), isTrue);
    expect(core.phase, SessionPhase.idle);
  },
      skip: library.isEmpty
          ? 'Pass the built client library with --dart-define=FLASHLINK_TEST_CLIENT_LIBRARY=...'
          : false);
}
