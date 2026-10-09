import 'package:flutter/material.dart';

import '../../shared/theme/chat_palette.dart';
import '../../shared/theme/grid.dart';
import '../../shared/theme/message_typography.dart';
import '../../shared/theme/theme_extensions.dart';
import 'date_formatters.dart';

/// Widest an own bubble may grow, as a share of the row. Desktop uses 75%;
/// a phone-width row needs a little more so short sentences don't wrap.
const chatOwnBubbleWidthFactor = 0.80;

/// Widest another person's (or agent's) bubble may grow, as a share of the
/// space right of the avatar column. Wider than own bubbles so long agent
/// reports stay readable; short messages still shrink to their content.
const chatOtherBubbleWidthFactor = 0.92;

const _bubbleRadius = Radius.circular(16);
const _bubbleTailRadius = Radius.circular(6);
const _bubbleMinWidth = 72.0;
const _metaLineHeight = 14.0;

/// One timeline message laid out as a chat bubble.
///
/// The signed-in user's own messages ([isOwn]) sit on the right with no
/// avatar or name. Everyone else, including agents the user owns, sits on the
/// left; the first message of a group ([showAuthor]) carries the avatar and a
/// name [header] inside the bubble. The time sits in the bubble's bottom-right
/// corner, and [below] (reactions) follows the bubble's side.
class ChatBubbleRow extends StatelessWidget {
  final bool isOwn;
  final bool showAuthor;
  final Widget avatar;
  final Widget? header;
  final List<Widget> content;
  final int createdAt;
  final bool edited;
  final Widget? below;
  final Key? bubbleKey;
  final Key? timestampKey;

  const ChatBubbleRow({
    super.key,
    required this.isOwn,
    required this.showAuthor,
    required this.avatar,
    required this.content,
    required this.createdAt,
    this.header,
    this.edited = false,
    this.below,
    this.bubbleKey,
    this.timestampKey,
  });

  @override
  Widget build(BuildContext context) {
    final palette = ChatPalette.of(context);
    final metaStyle = messageTimestampTextStyle.copyWith(
      fontSize: 11,
      height: _metaLineHeight / 11,
      color: context.colors.onSurfaceVariant,
    );

    return LayoutBuilder(
      builder: (context, constraints) {
        final avatarColumn = isOwn
            ? 0.0
            : messageAvatarSize + messageAvatarContentGap;
        final maxBubbleWidth = isOwn
            ? constraints.maxWidth * chatOwnBubbleWidthFactor
            : (constraints.maxWidth - avatarColumn) *
                  chatOtherBubbleWidthFactor;

        final bubble = ConstrainedBox(
          key: bubbleKey,
          constraints: BoxConstraints(
            minWidth: _bubbleMinWidth,
            maxWidth: maxBubbleWidth,
          ),
          child: DecoratedBox(
            decoration: BoxDecoration(
              color: isOwn ? palette.ownBubble : palette.otherBubble,
              borderRadius: BorderRadius.only(
                topLeft: _bubbleRadius,
                topRight: _bubbleRadius,
                bottomLeft: isOwn ? _bubbleRadius : _bubbleTailRadius,
                bottomRight: isOwn ? _bubbleTailRadius : _bubbleRadius,
              ),
              border: !isOwn && palette.otherBubbleOutline != null
                  ? Border.all(color: palette.otherBubbleOutline!, width: 0.5)
                  : null,
            ),
            child: Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Grid.twelve,
                vertical: Grid.half + Grid.quarter,
              ),
              // The time is pinned to the bubble's corner; the content keeps a
              // line of room for it so the two never overlap.
              child: Stack(
                clipBehavior: Clip.none,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(bottom: _metaLineHeight),
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        if (header != null)
                          Padding(
                            padding: const EdgeInsets.only(
                              bottom: Grid.quarter,
                            ),
                            child: header,
                          ),
                        ...content,
                      ],
                    ),
                  ),
                  Positioned(
                    right: 0,
                    bottom: 0,
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        if (edited) ...[
                          Text(
                            'edited',
                            style: metaStyle.copyWith(
                              fontStyle: FontStyle.italic,
                            ),
                          ),
                          const SizedBox(width: Grid.half),
                        ],
                        Text(
                          key: timestampKey,
                          formatMessageTime(createdAt),
                          maxLines: 1,
                          style: metaStyle,
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        );

        return Column(
          crossAxisAlignment: isOwn
              ? CrossAxisAlignment.end
              : CrossAxisAlignment.start,
          children: [
            Row(
              mainAxisAlignment: isOwn
                  ? MainAxisAlignment.end
                  : MainAxisAlignment.start,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (!isOwn) ...[
                  if (showAuthor)
                    avatar
                  else
                    const SizedBox(width: messageAvatarSize),
                  const SizedBox(width: messageAvatarContentGap),
                ],
                bubble,
              ],
            ),
            if (below != null)
              Padding(
                padding: EdgeInsets.only(left: avatarColumn),
                child: below,
              ),
          ],
        );
      },
    );
  }
}

/// The sender's name on the first bubble of a group.
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
        style: messageUsernameTextStyle.copyWith(
          fontSize: 14,
          color: context.colors.onSurface,
        ),
      ),
    );
  }
}
