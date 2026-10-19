import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/widgets/conversation_row_parts.dart';
import 'channel.dart';

/// A group's picture at any size: its first letter on the colour fixed by
/// its id (the Chats row avatar), with a lock badge when private. Forums keep
/// their forum icon and DMs their conversation icon.
class GroupAvatar extends StatelessWidget {
  const GroupAvatar({
    super.key,
    required this.channel,
    this.size = 24,
    this.iconColor,
  });

  final Channel channel;

  /// Diameter of the letter circle, or the icon size for forums and DMs.
  final double size;

  /// Colour of the forum or DM icon.
  final Color? iconColor;

  @override
  Widget build(BuildContext context) {
    if (channel.isDm || channel.isForum) {
      return Icon(
        channel.isDm
            ? LucideIcons.messagesSquare
            : LucideIcons.messageSquareText,
        size: size,
        color: iconColor,
      );
    }
    return ConversationLetterAvatar(
      seed: channel.id,
      label: channel.name,
      isPrivate: channel.isPrivate,
      size: size,
      avatarKey: null,
    );
  }
}
