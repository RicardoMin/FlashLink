import 'dart:io';
import 'dart:ui' as ui;

import 'package:flashlink/connection.dart';
import 'package:flashlink/main.dart';
import 'package:flashlink/player.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

const _output = String.fromEnvironment('FLASHLINK_PREVIEW_DIR');
const _font = String.fromEnvironment('FLASHLINK_PREVIEW_FONT');

class _PreviewPlayer implements Player {
  List<NearbyDevice> devices = [];
  @override
  Future<List<NearbyDevice>> discover() async => devices;
  @override
  Future<void> openSettings() async => throw UnsupportedError('Preview only');
  @override
  Future<void> launch(String host, String name, String id, int attempt,
          VoidCallback onOpened) async =>
      throw UnsupportedError('Preview only');
}

Future<void> _loadPreviewFonts() async {
  // Flutter widget tests normally use Ahem, which has no Chinese glyphs.
  final fonts = _font.isNotEmpty
      ? [_font]
      : [
          if (Platform.isMacOS) ...[
            '/System/Library/Fonts/Supplemental/Arial Unicode.ttf',
            '/System/Library/Fonts/Hiragino Sans GB.ttc',
          ],
          if (Platform.isWindows)
            '${Platform.environment['WINDIR']}\\Fonts\\msyh.ttc',
          if (Platform.isLinux)
            '/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc',
        ];
  final font = fonts.map(File.new).where((f) => f.existsSync()).firstOrNull;
  if (font == null) {
    throw StateError(
        '请通过 --dart-define=FLASHLINK_PREVIEW_FONT=<路径> 指定中文 TTF/OTF 字体。');
  }
  final data = ByteData.sublistView(await font.readAsBytes());
  for (final name in ['Ahem', 'Roboto']) {
    await (FontLoader(name)..addFont(Future.value(data))).load();
  }
  await (FontLoader('MaterialIcons')
        ..addFont(rootBundle.load('fonts/MaterialIcons-Regular.otf')))
      .load();
}

// Fixtures represent LAN discovery states. They never open a stream or read local device identities.
void main() {
  const desktop = NearbyDevice(
      id: 'fixture-desktop', name: '书房电脑', host: '192.0.2.1:47989');
  const laptop = NearbyDevice(
      id: 'fixture-laptop', name: 'ThinkPad 笔记本', host: '192.0.2.2:47989');
  const missing = NearbyDevice(
      id: 'fixture-missing', name: '客厅电脑', host: '192.0.2.3:47989');
  for (final tablet in [false, true]) {
    final layout = tablet ? 'tablet' : 'phone';
    testWidgets('render $layout device previews to FLASHLINK_PREVIEW_DIR',
        (tester) async {
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize =
          tablet ? const Size(1024, 640) : const Size(390, 760);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.runAsync(_loadPreviewFonts);
      final key = GlobalKey();
      final player = _PreviewPlayer();
      final controller = ConnectionController(
          () => throw UnsupportedError('Preview only'), player);
      addTearDown(controller.dispose);
      await tester.pumpWidget(RepaintBoundary(
          key: key, child: FlashLinkApp(controller: controller)));
      await tester.pumpAndSettle();
      Future<void> capture(String name) async {
        final boundary =
            key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
        await tester.runAsync(() async {
          final image = await boundary.toImage(pixelRatio: 1.5);
          try {
            final data = await image.toByteData(format: ui.ImageByteFormat.png);
            final directory = await Directory(_output).create(recursive: true);
            await File.fromUri(
                    directory.absolute.uri.resolve('android-$layout-$name.png'))
                .writeAsBytes(data!.buffer.asUint8List());
          } finally {
            image.dispose();
          }
        });
      }

      if (!tablet) await capture('empty');
      player.devices = [desktop, laptop, missing];
      await controller.discover();
      player.devices = [desktop, laptop];
      await controller.discover();
      await tester.pumpAndSettle();
      await capture('devices');
      if (!tablet) {
        await tester.tap(find.byTooltip('查看书房电脑详情'));
        await tester.pumpAndSettle();
        await capture('detail');
      }
    }, skip: _output.isEmpty);
  }
}
