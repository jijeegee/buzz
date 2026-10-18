import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../mentions/mention_bindings.dart';
import '../mentions/mention_tags.dart';
import '../profile/user_profile.dart';
import '../theme/theme.dart';
import 'avatar_image.dart';

/// The look shared by the Activity and Chats conversation rows: a colour
/// fixed per id, the row time, person and channel avatars, and `@name`
/// lookup for plain-text previews. Data and layout stay with each list.

/// Background of the Chats and People lists: white in a light theme (not the
/// theme's tinted surface), the theme surface in a dark one.
Color conversationListBackground(BuildContext context) =>
    Theme.of(context).brightness == Brightness.light
    ? Colors.white
    : context.colors.surface;

/// A colour fixed per id (a channel or thread).
Color conversationHue(String seed) {
  var hash = 0;
  for (final unit in seed.codeUnits) {
    hash = (hash * 31 + unit) & 0x7fffffff;
  }
  return HSLColor.fromAHSL(1, (hash % 360).toDouble(), 0.55, 0.48).toColor();
}

/// "3:05 PM" today, "Yesterday", else "Oct 12" ("Oct 12, 2025" in another
/// year).
String conversationTimestamp(int unixSeconds, {DateTime? now}) {
  final current = now ?? DateTime.now();
  final date = DateTime.fromMillisecondsSinceEpoch(unixSeconds * 1000);
  final today = DateTime(current.year, current.month, current.day);
  final day = DateTime(date.year, date.month, date.day);
  final dayDiff = today.difference(day).inDays;

  if (dayDiff == 0) {
    final hour = date.hour % 12 == 0 ? 12 : date.hour % 12;
    final minute = date.minute.toString().padLeft(2, '0');
    return '$hour:$minute ${date.hour < 12 ? 'AM' : 'PM'}';
  }
  if (dayDiff == 1) return 'Yesterday';

  const months = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec',
  ];
  final label = '${months[date.month - 1]} ${date.day}';
  return date.year == current.year ? label : '$label, ${date.year}';
}

/// First letter of [label], upper-cased; `#` when blank.
String conversationInitial(String label) {
  final trimmed = label.trim();
  return trimmed.isEmpty
      ? '#'
      : String.fromCharCode(trimmed.runes.first).toUpperCase();
}

/// A person's picture, or their initial on the primary container colour.
class ConversationPersonAvatar extends StatelessWidget {
  const ConversationPersonAvatar({
    super.key,
    required this.pubkey,
    required this.profile,
    required this.isAgent,
    this.size = activityAvatarSize,
    this.fontSize = 14,
  });

  final String pubkey;
  final UserProfile? profile;
  final bool isAgent;
  final double size;
  final double fontSize;

  @override
  Widget build(BuildContext context) {
    final initial =
        profile?.initial ?? (pubkey.isNotEmpty ? pubkey[0].toUpperCase() : '?');
    return AvatarImage(
      imageUrl: profile?.avatarUrl,
      radius: size / 2,
      backgroundColor: context.colors.primaryContainer,
      fallback: Text(
        initial,
        style: TextStyle(
          fontSize: fontSize,
          fontWeight: FontWeight.w600,
          color: context.colors.onPrimaryContainer,
        ),
      ),
      isAgent: isAgent,
    );
  }
}

/// A channel's picture: its first letter on its colour. [isThread] adds a
/// small speech-bubble badge (the Activity thread rows).
class ConversationLetterAvatar extends StatelessWidget {
  const ConversationLetterAvatar({
    super.key,
    required this.seed,
    required this.label,
    this.isThread = false,
    this.avatarKey = const ValueKey('activity-conversation-avatar'),
  });

  final String seed;
  final String label;
  final bool isThread;

  /// Key of the circle itself.
  final Key avatarKey;

  @override
  Widget build(BuildContext context) => SizedBox.square(
    dimension: activityAvatarSize,
    child: Stack(
      clipBehavior: Clip.none,
      children: [
        CircleAvatar(
          key: avatarKey,
          radius: activityAvatarSize / 2,
          backgroundColor: conversationHue(seed),
          child: Text(
            conversationInitial(label),
            style: const TextStyle(
              fontSize: 14,
              fontWeight: FontWeight.w600,
              color: Colors.white,
            ),
          ),
        ),
        if (isThread)
          Positioned(
            right: -2,
            bottom: -2,
            child: Container(
              width: 16,
              height: 16,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: context.colors.surface,
                border: Border.all(color: context.colors.outlineVariant),
              ),
              child: Icon(
                LucideIcons.messagesSquare,
                size: 10,
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ),
      ],
    ),
  );
}

/// Circle size in a two-circle avatar (desktop 28 of 36, scaled to 42).
const conversationPairCircleSize = 33.0;

/// Width of a two-circle avatar: the circles overlap by 15.
const conversationPairWidth = 51.0;

/// How far a two-circle avatar reaches past each side of the avatar column.
/// Centred on the column, it stays clear of the row text (which starts 8
/// after the column, as on single-avatar rows) and still lines up with it.
const conversationPairOverhang =
    (conversationPairWidth - activityAvatarSize) / 2;

/// Two same-size circles side by side, overlapping, centred on the avatar
/// column and offset only sideways: [back] on the left, [front] on the right
/// with a ring of [ringColor]. Lays out [activityAvatarSize] wide; the
/// circles reach [conversationPairOverhang] past each side.
class ConversationAvatarPair extends StatelessWidget {
  const ConversationAvatarPair({
    super.key,
    required this.back,
    required this.front,
    required this.ringColor,
  });

  final Widget back;
  final Widget front;
  final Color ringColor;

  @override
  Widget build(BuildContext context) {
    const top = (activityAvatarSize - conversationPairCircleSize) / 2;
    return SizedBox(
      width: activityAvatarSize,
      height: activityAvatarSize,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Positioned(
            left: -conversationPairOverhang,
            top: top,
            width: conversationPairCircleSize,
            height: conversationPairCircleSize,
            child: back,
          ),
          Positioned(
            left:
                conversationPairWidth -
                conversationPairOverhang -
                conversationPairCircleSize -
                2,
            top: top - 2,
            width: conversationPairCircleSize + 4,
            height: conversationPairCircleSize + 4,
            child: DecoratedBox(
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                border: Border.all(color: ringColor, width: 2),
              ),
              child: Padding(padding: const EdgeInsets.all(2), child: front),
            ),
          ),
        ],
      ),
    );
  }
}

/// A thread's picture (desktop's A design): the channel's circle behind with
/// its letter kept to the left so the front circle never covers it, and the
/// thread's own circle and letter in front.
class ConversationThreadAvatar extends StatelessWidget {
  const ConversationThreadAvatar({
    super.key,
    required this.channelSeed,
    required this.channelLabel,
    required this.threadSeed,
    required this.threadLabel,
    required this.ringColor,
  });

  final String channelSeed;
  final String channelLabel;
  final String threadSeed;
  final String threadLabel;
  final Color ringColor;

  @override
  Widget build(BuildContext context) => ConversationAvatarPair(
    ringColor: ringColor,
    back: Container(
      key: const ValueKey('conversation-thread-avatar-channel'),
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: conversationHue(channelSeed),
      ),
      alignment: Alignment.centerLeft,
      // Clear of the front circle's ring (which starts at 16).
      padding: const EdgeInsets.only(left: 6),
      child: Text(
        conversationInitial(channelLabel),
        style: const TextStyle(
          fontSize: 11,
          fontWeight: FontWeight.w600,
          color: Colors.white,
          height: 1,
        ),
      ),
    ),
    front: Container(
      key: const ValueKey('conversation-thread-avatar-thread'),
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: conversationHue(threadSeed),
      ),
      alignment: Alignment.center,
      child: Text(
        conversationInitial(threadLabel),
        style: const TextStyle(
          fontSize: 14,
          fontWeight: FontWeight.w600,
          color: Colors.white,
          height: 1,
        ),
      ),
    ),
  );
}

final _qualifiedMention = RegExp(
  r'@([^@\r\n()]+?) \(([0-9a-f]{64})\)(?: (?:[1-9][0-9]+|[2-9]))?',
  caseSensitive: false,
);

/// Shows each `@mention` in a plain-text preview by the person's current
/// name. [names] maps lowercase pubkeys to names already on this device
/// (cached profiles); nothing is fetched. A mention is renamed only when its
/// text binds to exactly one tagged person; the `@Name (pubkey)` form drops
/// the pubkey even without a cached name.
String resolvePreviewMentions(
  String text,
  List<List<String>> tags,
  Map<String, String> names,
) {
  final tagged = mentionedPubkeysFromTags(tags);
  var result = text;
  if (tagged.isNotEmpty) {
    final taggedNames = {
      for (final pubkey in tagged)
        if (names[pubkey]?.trim().isNotEmpty == true)
          pubkey: names[pubkey]!.trim(),
    };
    final bindings = renderedMentionBindings(result, taggedNames, tagged);
    for (final range in mentionOccurrences(result, bindings.keys).reversed) {
      final keys = bindings[range.label.toLowerCase()] ?? const <String>{};
      final name = keys.length == 1 ? taggedNames[keys.first] : null;
      if (name == null) continue;
      result = result.replaceRange(range.start, range.end, '@$name');
    }
  }
  return result.replaceAllMapped(_qualifiedMention, (match) {
    final pubkey = match.group(2)!.toLowerCase();
    final name = tagged.contains(pubkey) ? names[pubkey]?.trim() : null;
    return '@${name == null || name.isEmpty ? match.group(1)!.trim() : name}';
  });
}

/// Height of a conversation row's title line (the title style's line, scaled
/// with the system text size).
double conversationTitleLineHeight(BuildContext context) =>
    MediaQuery.textScalerOf(
      context,
    ).scale(activityUsernameTextStyle.fontSize!) *
    activityUsernameTextStyle.height!;

/// The Activity "Channels + Threads" row, shared with Chats: [avatar] | a
/// title line over the [preview], gutter-wide sides and 12 above and below.
///
/// The title line is [title] filling the width, then [titleTrailing] (the
/// Activity time and dot). With a [side] column (Chats: the time over the
/// unread badges) the title and preview stop at the column, which starts
/// level with the title line.
class ConversationRowLayout extends StatelessWidget {
  const ConversationRowLayout({
    super.key,
    required this.avatar,
    required this.title,
    this.titleTrailing = const [],
    this.preview,
    this.side,
    this.startInset = 0,
  });

  final Widget avatar;

  /// The title line's start; it fills the line. Activity's message view puts
  /// its sender and context lines here.
  final Widget title;
  final List<Widget> titleTrailing;
  final Widget? preview;
  final Widget? side;

  /// Extra start padding (Chats' By channel thread indent).
  final double startInset;

  @override
  Widget build(BuildContext context) {
    final body = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (titleTrailing.isEmpty)
          title
        else
          Row(
            children: [
              Expanded(child: title),
              const SizedBox(width: Grid.xxs),
              ...titleTrailing,
            ],
          ),
        if (preview case final preview?) ...[
          const SizedBox(height: conversationTitlePreviewGap),
          preview,
        ],
      ],
    );
    return Padding(
      padding: EdgeInsetsDirectional.fromSTEB(
        Grid.gutter + startInset,
        Grid.twelve,
        Grid.gutter,
        Grid.twelve,
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          avatar,
          const SizedBox(width: messageAvatarContentGap),
          Expanded(child: body),
          if (side case final side?) ...[const SizedBox(width: Grid.xxs), side],
        ],
      ),
    );
  }
}

/// Space between a conversation row's title line and its preview.
const conversationTitlePreviewGap = Grid.half;
