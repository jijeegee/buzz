import 'package:flutter/material.dart';

import '../../shared/theme/buzz_theme.dart';
import '../../shared/theme/grid.dart';
import '../../shared/theme/message_typography.dart';
import '../../shared/theme/theme_extensions.dart';
import 'date_formatters.dart';

/// Widest an own bubble may grow, as a share of the row (desktop's value).
const chatOwnBubbleWidthFactor = 0.75;

/// Widest another person's (or agent's) bubble may grow, as a share of the
/// space right of the avatar column. Narrower than desktop's 92% so the time
/// fits beside the bubble.
const chatOtherBubbleWidthFactor = 0.80;

/// Vertical padding inside a bubble.
const chatBubbleVerticalPadding = 10.0;

/// Timeline gutters, KakaoTalk-tight: the avatar column hugs the left edge and
/// own bubbles stop a little short of the right edge.
const chatListLeftInset = 8.0;
const chatListRightInset = 12.0;

const _bubbleRadius = Radius.circular(16);
const _bubbleTailRadius = Radius.circular(6);

/// Bubble fills, mirroring desktop's `--buzz-bubble-own` /
/// `--buzz-bubble-other` / `--buzz-bubble-other-shadow` in
/// `desktop/src/shared/styles/globals/theme.css`. Only the bubbles are
/// painted; the timeline background is left to the theme.
class ChatBubbleColors {
  final Color own;
  final Color other;
  final List<BoxShadow> otherShadow;

  const ChatBubbleColors({
    required this.own,
    required this.other,
    this.otherShadow = const [],
  });

  static const _buzzLight = ChatBubbleColors(
    own: Color(0xFFF3EB9E),
    other: Color(0xFFFFFFFF),
    otherShadow: [
      BoxShadow(color: Color(0x0D000000), spreadRadius: 1),
      BoxShadow(color: Color(0x0F000000), offset: Offset(0, 1), blurRadius: 2),
    ],
  );

  static const _buzzDark = ChatBubbleColors(
    own: Color(0xFF57511F),
    other: Color(0xFF262B33),
  );

  static ChatBubbleColors of(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final isDark = scheme.brightness == Brightness.dark;
    if (isBuzzThemeContext(context)) return isDark ? _buzzDark : _buzzLight;
    // Desktop's default: `hsl(var(--muted))` and `hsl(var(--primary) / 0.15)`.
    return ChatBubbleColors(
      own: Color.alphaBlend(
        scheme.primary.withValues(alpha: 0.15),
        scheme.surface,
      ),
      other: scheme.surfaceContainerHighest,
    );
  }
}

/// One timeline message laid out as a chat bubble.
///
/// The signed-in user's own messages ([isOwn]) sit on the right with no
/// avatar or name. Everyone else, including agents the user owns, sits on the
/// left behind [leading] (the avatar, or an empty avatar-width slot), with the
/// group's [header] above the first bubble. The time sits beside the bubble
/// on its outer side, level with its bottom edge, and only when [showTime] is
/// set (the last message of a group). [ownLeading] (the speaker control) sits in the free space left
/// of an own bubble so it never narrows the bubble.
class ChatBubbleRow extends StatelessWidget {
  final bool isOwn;
  final Widget leading;
  final Widget? header;
  final Widget? ownLeading;
  final Widget child;
  final bool showTime;
  final int createdAt;
  final bool edited;
  final Key? bubbleKey;
  final Key? timestampKey;

  const ChatBubbleRow({
    super.key,
    required this.isOwn,
    required this.leading,
    required this.child,
    required this.showTime,
    required this.createdAt,
    this.header,
    this.ownLeading,
    this.edited = false,
    this.bubbleKey,
    this.timestampKey,
  });

  @override
  Widget build(BuildContext context) {
    final colors = ChatBubbleColors.of(context);
    final metaColor = context.colors.onSurfaceVariant;

    final bubble = DecoratedBox(
      key: bubbleKey,
      decoration: BoxDecoration(
        color: isOwn ? colors.own : colors.other,
        borderRadius: BorderRadius.only(
          topLeft: _bubbleRadius,
          topRight: _bubbleRadius,
          bottomLeft: isOwn ? _bubbleRadius : _bubbleTailRadius,
          bottomRight: isOwn ? _bubbleTailRadius : _bubbleRadius,
        ),
        boxShadow: isOwn ? null : colors.otherShadow,
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: Grid.twelve,
          vertical: chatBubbleVerticalPadding,
        ),
        child: child,
      ),
    );

    final meta = showTime || edited
        ? Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (edited)
                Text(
                  '(edited)',
                  style: chatTimestampTextStyle.copyWith(
                    color: metaColor,
                    fontStyle: FontStyle.italic,
                  ),
                ),
              if (edited && showTime) const SizedBox(width: Grid.half),
              if (showTime)
                Text(
                  key: timestampKey,
                  formatMessageTime(createdAt),
                  maxLines: 1,
                  style: chatTimestampTextStyle.copyWith(color: metaColor),
                ),
            ],
          )
        : null;

    // The time sits beside the bubble on its outer side, level with the
    // bubble's bottom edge: right of others' bubbles, left of own bubbles.
    Widget bubbleWithMeta(double maxBubbleWidth) => Row(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        if (isOwn && meta != null) ...[meta, const SizedBox(width: Grid.half)],
        Flexible(
          child: ConstrainedBox(
            constraints: BoxConstraints(maxWidth: maxBubbleWidth),
            child: bubble,
          ),
        ),
        if (!isOwn && meta != null) ...[const SizedBox(width: Grid.half), meta],
      ],
    );

    if (isOwn) {
      return LayoutBuilder(
        builder: (context, constraints) => Row(
          mainAxisAlignment: MainAxisAlignment.end,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (ownLeading != null) ...[
              ownLeading!,
              const SizedBox(width: Grid.half),
            ],
            Flexible(
              child: bubbleWithMeta(
                constraints.maxWidth * chatOwnBubbleWidthFactor,
              ),
            ),
          ],
        ),
      );
    }

    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        leading,
        const SizedBox(width: messageAvatarContentGap),
        Expanded(
          child: LayoutBuilder(
            builder: (context, constraints) => Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ?header,
                bubbleWithMeta(
                  constraints.maxWidth * chatOtherBubbleWidthFactor,
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

/// The sender's name above the first bubble of a group.
class ChatBubbleAuthor extends StatelessWidget {
  final String displayName;
  final VoidCallback? onTap;

  const ChatBubbleAuthor({super.key, required this.displayName, this.onTap});

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Text(
        displayName,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: chatAuthorNameTextStyle.copyWith(
          color: context.colors.onSurface,
        ),
      ),
    );
  }
}
