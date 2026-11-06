import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';

/// The Kuzz "K", painted so it appears on the first frame at any size.
///
/// The geometry matches `KUZZ_MARK_PATH` in the desktop app. Width follows
/// from [height].
class KuzzMark extends StatelessWidget {
  /// The rendered height of the K.
  final double height;

  /// The color of the K.
  final Color color;

  const KuzzMark({required this.height, required this.color, super.key});

  static const _viewBoxWidth = 1413.0;
  static const _viewBoxHeight = 1490.0;

  @override
  Widget build(BuildContext context) {
    return CustomPaint(
      size: Size(height * _viewBoxWidth / _viewBoxHeight, height),
      painter: _KuzzMarkPainter(color),
    );
  }
}

class _KuzzMarkPainter extends CustomPainter {
  final Color color;

  const _KuzzMarkPainter(this.color);

  // Outlined from Inter Black with the origin at the viewBox's top-left.
  static final _path = Path()
    ..addPolygon(const [
      Offset(307, 1277),
      Offset(307, 772),
      Offset(904, 0),
      Offset(1385, 0),
      Offset(762, 767),
      Offset(724, 767),
    ], true)
    ..addRect(const Rect.fromLTRB(0, 0, 416, 1490))
    ..addPolygon(const [
      Offset(937, 1490),
      Offset(578, 891),
      Offset(826, 579),
      Offset(1413, 1490),
    ], true);

  @override
  void paint(Canvas canvas, Size size) {
    canvas.scale(
      size.width / KuzzMark._viewBoxWidth,
      size.height / KuzzMark._viewBoxHeight,
    );
    canvas.drawPath(_path, Paint()..color = color);
  }

  @override
  bool shouldRepaint(_KuzzMarkPainter oldDelegate) =>
      color != oldDelegate.color;
}

/// The loading state of [KuzzMark]: a slow pulse matching the desktop app.
///
/// Stays still when [active] is false or the system asks for reduced motion.
class KuzzLoadingMark extends HookWidget {
  /// The rendered height of the K.
  final double height;

  /// The color of the K.
  final Color color;

  /// Whether the mark pulses.
  final bool active;

  const KuzzLoadingMark({
    required this.height,
    required this.color,
    this.active = true,
    super.key,
  });

  static const _minScale = 0.94;
  static const _maxScale = 1.06;

  @override
  Widget build(BuildContext context) {
    final pulses = active && !MediaQuery.disableAnimationsOf(context);
    final controller = useAnimationController(
      duration: const Duration(milliseconds: 900),
    );
    useEffect(() {
      if (pulses) {
        // Start at the resting size so the mark does not jump.
        controller
          ..value = 0.5
          ..repeat(reverse: true);
      } else {
        controller
          ..stop()
          ..reset();
      }
      return null;
    }, [controller, pulses]);

    final mark = KuzzMark(height: height, color: color);
    if (!pulses) return mark;
    return ScaleTransition(
      scale: controller
          .drive(CurveTween(curve: Curves.easeInOut))
          .drive(Tween(begin: _minScale, end: _maxScale)),
      child: mark,
    );
  }
}
