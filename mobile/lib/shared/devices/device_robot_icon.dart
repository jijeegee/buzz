import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import 'device_robot.dart';

/// A device's robot, or the default bot glyph when [variant] is `null`.
/// Decorative unless [semanticLabel] is given.
class DeviceRobotIcon extends StatelessWidget {
  const DeviceRobotIcon({
    super.key,
    required this.variant,
    this.size = 20,
    this.fallbackColor,
    this.semanticLabel,
  });

  final DeviceRobotVariant? variant;
  final double size;

  /// Colour of the default bot glyph (defaults to the ambient icon colour).
  final Color? fallbackColor;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final robot = variant;
    final Widget glyph = robot == null
        ? Icon(LucideIcons.bot, size: size, color: fallbackColor)
        : CustomPaint(
            key: ValueKey('device-robot-${robot.tag}'),
            size: Size.square(size),
            painter: DeviceRobotPainter(robot),
          );
    final label = semanticLabel;
    return label == null
        ? ExcludeSemantics(child: glyph)
        : Semantics(
            label: label,
            image: true,
            child: ExcludeSemantics(child: glyph),
          );
  }
}

/// Paints one robot variant scaled from the 24×24 grid.
class DeviceRobotPainter extends CustomPainter {
  DeviceRobotPainter(this.variant);

  final DeviceRobotVariant variant;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = size.shortestSide / 24;
    canvas.save();
    canvas.scale(scale);
    final stroke = Paint()
      ..color = variant.color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round
      ..isAntiAlias = true;
    final fill = Paint()
      ..color = variant.color
      ..style = PaintingStyle.fill
      ..isAntiAlias = true;
    for (final shape in deviceRobotGeometry[variant.shape]!) {
      switch (shape) {
        case RobotRect(:final x, :final y, :final w, :final h, :final r):
          canvas.drawRRect(
            RRect.fromRectAndRadius(
              Rect.fromLTWH(x, y, w, h),
              Radius.circular(r),
            ),
            stroke,
          );
        case RobotLine(:final x1, :final y1, :final x2, :final y2):
          canvas.drawLine(Offset(x1, y1), Offset(x2, y2), stroke);
        case RobotCircle(:final cx, :final cy, :final r):
          canvas.drawCircle(Offset(cx, cy), r, stroke);
        case RobotDot(:final cx, :final cy, :final r):
          canvas.drawCircle(Offset(cx, cy), r, fill);
        case RobotPoly(:final points):
          final path = Path()..moveTo(points[0], points[1]);
          for (var i = 2; i < points.length; i += 2) {
            path.lineTo(points[i], points[i + 1]);
          }
          canvas.drawPath(path..close(), stroke);
      }
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(DeviceRobotPainter oldDelegate) =>
      oldDelegate.variant != variant;
}

/// [child] (an agent avatar) with its device robot in the bottom-right
/// corner on a surface-coloured disc; [child] unchanged when [robot] is
/// `null`. The badge is decorative: the avatar's owner keeps the label.
class DeviceRobotAvatarBadge extends StatelessWidget {
  const DeviceRobotAvatarBadge({
    super.key,
    required this.robot,
    required this.child,
    this.badgeSize = 18,
  });

  final DeviceRobotVariant? robot;
  final Widget child;
  final double badgeSize;

  @override
  Widget build(BuildContext context) {
    final variant = robot;
    if (variant == null) return child;
    return Stack(
      clipBehavior: Clip.none,
      children: [
        child,
        Positioned(
          right: -badgeSize * 0.25,
          bottom: -badgeSize * 0.25,
          child: Container(
            key: ValueKey('device-robot-badge-${variant.tag}'),
            width: badgeSize,
            height: badgeSize,
            padding: EdgeInsets.all(badgeSize * 0.1),
            decoration: BoxDecoration(
              color: Theme.of(context).colorScheme.surface,
              shape: BoxShape.circle,
            ),
            child: DeviceRobotIcon(variant: variant, size: badgeSize * 0.8),
          ),
        ),
      ],
    );
  }
}
