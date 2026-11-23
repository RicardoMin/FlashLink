import 'dart:math' as math;

import 'package:flutter/material.dart';

/// Original, static device artwork. A computer's desktop is private until paired.
class DeviceCover extends StatelessWidget {
  const DeviceCover({super.key, this.available = true});
  final bool available;

  @override
  Widget build(BuildContext context) => ExcludeSemantics(
        child: CustomPaint(
          painter: _Landscape(available),
          size: Size.infinite,
        ),
      );
}

class _Landscape extends CustomPainter {
  const _Landscape(this.available);
  final bool available;

  @override
  void paint(Canvas canvas, Size size) {
    if (size.isEmpty) return;
    final rect = Offset.zero & size;
    final sky = Paint()
      ..shader = const LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [Color(0xffd9f3ff), Color(0xff8ccfff), Color(0xff5a96ee)],
      ).createShader(rect);
    canvas.drawRect(rect, sky);
    canvas.save();
    canvas.clipRect(rect);
    // Use a single landscape composition at both thumbnail and banner sizes.
    final scale = math.max(size.width / 400, size.height / 218);
    canvas.translate(
        (size.width - 400 * scale) / 2, (size.height - 218 * scale) / 2);
    canvas.scale(scale);
    final layers = [
      (
        Path()
          ..moveTo(-28, 144)
          ..cubicTo(53, 130, 127, 33, 171, 42)
          ..cubicTo(224, 52, 239, 148, 322, 144)
          ..lineTo(416, 168),
        const [Color(0xff9adaff), Color(0xff4088e8), Color(0xff4562b8)]
      ),
      (
        Path()
          ..moveTo(-24, 158)
          ..cubicTo(65, 129, 144, 114, 221, 67)
          ..cubicTo(282, 29, 297, 25, 323, 42)
          ..cubicTo(361, 64, 380, 104, 420, 94),
        const [Color(0xffdbf6ff), Color(0xff71c5fa), Color(0xff3169dc)]
      ),
      (
        Path()
          ..moveTo(-24, 164)
          ..cubicTo(93, 134, 131, 62, 185, 60)
          ..cubicTo(230, 60, 258, 125, 320, 137)
          ..cubicTo(351, 144, 399, 127, 420, 106),
        const [Color(0xff51aeff), Color(0xff2265e2), Color(0xff183eaf)]
      ),
      (
        Path()
          ..moveTo(-24, 189)
          ..cubicTo(83, 221, 151, 136, 206, 115)
          ..cubicTo(263, 94, 283, 118, 340, 104)
          ..cubicTo(382, 93, 404, 69, 420, 67),
        const [Color(0xff76ccff), Color(0xff3979eb), Color(0xff2248b0)]
      ),
      (
        Path()
          ..moveTo(-24, 157)
          ..cubicTo(66, 142, 99, 187, 186, 183)
          ..cubicTo(269, 179, 303, 138, 363, 133)
          ..cubicTo(390, 130, 402, 135, 420, 128),
        const [Color(0xff438df1), Color(0xff255cc9), Color(0xff162e83)]
      ),
    ];
    for (final (path, colors) in layers) {
      path.lineTo(425, 240);
      path.lineTo(-30, 240);
      path.close();
      canvas.drawPath(
          path,
          Paint()
            ..shader = LinearGradient(
                    begin: Alignment.topLeft,
                    end: Alignment.bottomRight,
                    colors: colors)
                .createShader(path.getBounds()));
    }
    canvas.restore();
    if (!available) {
      canvas.drawRect(rect, Paint()..color = const Color(0x91e5e9ef));
    }
  }

  @override
  bool shouldRepaint(_Landscape oldDelegate) =>
      available != oldDelegate.available;
}
