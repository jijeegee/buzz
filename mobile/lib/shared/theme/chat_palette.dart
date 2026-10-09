import 'package:flutter/material.dart';

import 'buzz_theme.dart';

/// Chat timeline colors: the canvas behind the bubbles, other people's
/// bubbles, and the signed-in user's own bubbles.
///
/// Mirrors desktop's `--buzz-chat-canvas` / `--buzz-bubble-*` custom
/// properties in `desktop/src/shared/styles/globals/theme.css`. Buzz paints
/// them from the app icon's yellow; other themes derive them from their own
/// color scheme.
class ChatPalette {
  final Color canvas;
  final Color otherBubble;
  final Color ownBubble;

  /// Translucent fill for code, tables, and quote previews inside a bubble so
  /// they stay distinct on both bubble colors.
  final Color inset;

  /// Hairline outline that lifts a white bubble off a light canvas.
  final Color? otherBubbleOutline;

  const ChatPalette({
    required this.canvas,
    required this.otherBubble,
    required this.ownBubble,
    required this.inset,
    this.otherBubbleOutline,
  });

  static const _buzzLight = ChatPalette(
    canvas: Color(0xFFF1F2EA),
    otherBubble: Color(0xFFFFFFFF),
    ownBubble: Color(0xFFF3EB9E),
    inset: Color(0x0D000000),
    otherBubbleOutline: Color(0x14000000),
  );

  static const _buzzDark = ChatPalette(
    canvas: Color(0xFF14181F),
    otherBubble: Color(0xFF262B33),
    ownBubble: Color(0xFF57511F),
    inset: Color(0x3D000000),
  );

  static ChatPalette of(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final isDark = scheme.brightness == Brightness.dark;
    if (isBuzzThemeContext(context)) return isDark ? _buzzDark : _buzzLight;
    return ChatPalette(
      canvas: scheme.surface,
      otherBubble: scheme.surfaceContainerHighest,
      ownBubble: Color.alphaBlend(
        scheme.primary.withValues(alpha: 0.15),
        scheme.surface,
      ),
      inset: isDark ? const Color(0x3D000000) : const Color(0x0D000000),
    );
  }
}
