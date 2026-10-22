import 'package:flutter/foundation.dart';

import '../../shared/l10n/l10n.dart';

@immutable
class FeedItem {
  final String id;
  final int kind;
  final String pubkey;
  final String content;
  final int createdAt;
  final String? channelId;
  final String channelName;
  final List<List<String>> tags;
  final String
  category; // "mention", "needs_action", "activity", "agent_activity"

  const FeedItem({
    required this.id,
    required this.kind,
    required this.pubkey,
    required this.content,
    required this.createdAt,
    required this.channelId,
    required this.channelName,
    required this.tags,
    required this.category,
  });

  factory FeedItem.fromJson(Map<String, dynamic> json) => FeedItem(
    id: json['id'] as String,
    kind: json['kind'] as int,
    pubkey: json['pubkey'] as String,
    content: json['content'] as String,
    createdAt: json['created_at'] as int,
    channelId: json['channel_id'] as String?,
    channelName: (json['channel_name'] as String?) ?? '',
    tags:
        (json['tags'] as List<dynamic>?)
            ?.map((t) => (t as List<dynamic>).map((e) => e as String).toList())
            .toList() ??
        const [],
    category: json['category'] as String,
  );

  /// The message whose thread directly contains this item, when this event is
  /// a reply. For nested replies this is the direct parent, not the outer root.
  String? get threadHeadId {
    for (final tag in tags) {
      if (tag.length >= 4 && tag[0] == 'e' && tag[3] == 'reply') {
        return tag[1];
      }
    }
    return null;
  }

  /// Human-readable headline based on event kind and category.
  String get headline {
    switch (kind) {
      case 45001:
        return appL10n.activityHeadlineForumPost;
      case 45003:
        return appL10n.activityHeadlineForumReply;
      case 46010:
        return appL10n.activityHeadlineApprovalRequested;
      case 43001:
        return appL10n.activityHeadlineJobRequested;
      case 43002:
        return appL10n.activityHeadlineJobAccepted;
      case 43003:
        return appL10n.activityHeadlineProgressUpdate;
      case 43004:
        return appL10n.activityHeadlineJobResult;
      case 43005:
        return appL10n.activityHeadlineJobCancelled;
      case 43006:
        return appL10n.activityHeadlineJobFailed;
      default:
        if (category == 'mention') return appL10n.activityHeadlineMention;
        if (category == 'agent_activity') {
          return appL10n.activityHeadlineAgentUpdate;
        }
        return appL10n.activityHeadlineGroupUpdate;
    }
  }

  /// Trimmed content, with a fallback for empty events.
  String get displayContent {
    final trimmed = content.trim();
    if (trimmed.isNotEmpty) return trimmed;
    if (kind == 46010) return appL10n.activityApprovalWaiting;
    return appL10n.activityNoDetails;
  }
}

/// Parsed response from GET /api/feed.
@immutable
class HomeFeedResponse {
  final List<FeedItem> mentions;
  final List<FeedItem> needsAction;
  final List<FeedItem> activity;
  final List<FeedItem> agentActivity;

  HomeFeedResponse({
    required this.mentions,
    required this.needsAction,
    required this.activity,
    required this.agentActivity,
  });

  factory HomeFeedResponse.fromJson(Map<String, dynamic> json) {
    final feed = json['feed'] as Map<String, dynamic>;
    return HomeFeedResponse(
      mentions: _parseItems(feed['mentions']),
      needsAction: _parseItems(feed['needs_action']),
      activity: _parseItems(feed['activity']),
      agentActivity: _parseItems(feed['agent_activity']),
    );
  }

  /// All items merged into a single list, sorted newest-first.
  late final List<FeedItem> all = () {
    final items = [...mentions, ...needsAction, ...activity, ...agentActivity];
    items.sort((a, b) => b.createdAt.compareTo(a.createdAt));
    return List<FeedItem>.unmodifiable(items);
  }();

  bool get isEmpty =>
      mentions.isEmpty &&
      needsAction.isEmpty &&
      activity.isEmpty &&
      agentActivity.isEmpty;
}

List<FeedItem> _parseItems(dynamic json) {
  if (json == null) return const [];
  return (json as List<dynamic>)
      .cast<Map<String, dynamic>>()
      .map(FeedItem.fromJson)
      .toList();
}
