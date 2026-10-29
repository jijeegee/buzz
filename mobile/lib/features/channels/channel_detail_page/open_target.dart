part of '../channel_detail_page.dart';

/// Where a channel page is in opening its timeline at a message.
enum _OpenPhase {
  /// The timeline opens at the newest rows, as it always did.
  none,

  /// Waiting for the window around the message; nothing is drawn yet.
  opening,

  /// The window is in; the list settles its first position out of sight.
  positioning,

  /// The list shows the window opened at the message.
  opened,
}

/// The messages a page may open at; the oldest one found is used.
class _OpenTarget {
  const _OpenTarget({
    required this.ids,
    required this.isUnread,
    this.knownCreatedAt = const {},
    this.edge,
  });

  final Set<String> ids;
  final bool isUnread;
  final Map<String, int> knownCreatedAt;

  /// Where a saved position had its row, when the target is one.
  final double? edge;
}

/// The message the timeline window was opened at.
class _OpenedAt {
  const _OpenedAt({
    required this.messageId,
    required this.createdAt,
    required this.isUnread,
    this.edge,
  });

  final String messageId;
  final int createdAt;

  /// The first unread message: it gets the "New messages" divider and sits
  /// at the top of the timeline rather than near its middle.
  final bool isUnread;

  /// A saved position's row goes back where it was: its bottom edge as a
  /// fraction of the timeline's height from the bottom.
  final double? edge;
}
