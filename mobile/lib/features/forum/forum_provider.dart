import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../channels/channel_management_provider.dart';
import '../channels/channel_messages_provider.dart';
import '../channels/failed_local_messages_provider.dart';
import '../../shared/custom_emoji/custom_emoji.dart';
import '../../shared/custom_emoji/custom_emoji_provider.dart';
import 'forum_local_sends.dart';
import 'forum_models.dart';

/// Fetches forum posts (kind:45001) for a channel from the relay.
///
/// Posts are top-level events tagged `#h:<channelId>`. Invalidate to refresh
/// (e.g. after creating a new post).
final forumPostsProvider = FutureProvider.family<ForumPostsResponse, String>((
  ref,
  channelId,
) async {
  final session = ref.watch(relaySessionProvider.notifier);
  final events = await session.fetchHistory(
    NostrFilters.forumPosts(channelId, limit: 50),
  );
  return ForumPostsResponse.fromEvents(events);
});

/// Fetches a forum thread (root post + replies) from the relay.
final forumThreadProvider =
    FutureProvider.family<
      ForumThreadResponse,
      ({String channelId, String eventId})
    >((ref, args) async {
      final session = ref.watch(relaySessionProvider.notifier);

      final results = await Future.wait([
        // Root event lookup by id.
        session.fetchHistory(
          NostrFilter(
            kinds: const [9, 40002, 45001, 45003],
            ids: [args.eventId],
            limit: 1,
          ),
        ),
        // Replies pointing at this root.
        session.fetchHistory(
          NostrFilters.forumThread(args.eventId, args.channelId),
        ),
      ]);

      final rootEvents = results[0];
      final replyEvents = results[1];
      if (rootEvents.isEmpty) {
        throw Exception('Forum thread not found: ${args.eventId}');
      }
      return ForumThreadResponse.fromEvents(
        root: rootEvents.first,
        replies: replyEvents,
      );
    });

/// A forum event delivery bound to the community where composition began.
///
/// Attachment uploads can outlive their route. Capturing the relay identity,
/// signing key, and emoji palette prevents a queued draft from being delivered
/// to a different community after the user switches relays.
class ForumEventDelivery {
  final ProviderContainer _container;
  final String _relayUrl;
  final String? _nsec;
  final SignedEventRelay _relay;
  final List<CustomEmoji> _customEmoji;

  ForumEventDelivery._({
    required ProviderContainer container,
    required String relayUrl,
    required String? nsec,
    required SignedEventRelay relay,
    required List<CustomEmoji> customEmoji,
  }) : _container = container,
       _relayUrl = relayUrl,
       _nsec = nsec,
       _relay = relay,
       _customEmoji = customEmoji;

  /// Captures the active community dependencies for a future delivery.
  factory ForumEventDelivery.capture(ProviderContainer container) {
    final config = container.read(relayConfigProvider);
    return ForumEventDelivery._(
      container: container,
      relayUrl: config.baseUrl,
      nsec: config.nsec,
      relay: SignedEventRelay.forConfig(
        session: container.read(relaySessionProvider.notifier),
        config: config,
      ),
      customEmoji: List<CustomEmoji>.unmodifiable(
        container.read(customEmojiListProvider),
      ),
    );
  }

  /// Creates a new forum post (kind:45001).
  ///
  /// The post is shown as soon as it is signed ([onShown] then runs) and
  /// sends like a channel message: a publish failure marks it failed for
  /// retry or delete and completes normally. Only failures before it is
  /// shown throw.
  Future<void> createPost({
    required String channelId,
    required String content,
    List<String> mentionPubkeys = const [],
    List<List<String>> mediaTags = const [],
    void Function()? onShown,
  }) => _submit(
    kind: EventKind.forumPost,
    channelId: channelId,
    content: content,
    mentionPubkeys: mentionPubkeys,
    mediaTags: mediaTags,
    onShown: onShown,
  );

  /// Creates a reply to a forum post (kind:45003), shown and sent like
  /// [createPost].
  Future<void> createReply({
    required String channelId,
    required String parentEventId,
    required String content,
    List<String> mentionPubkeys = const [],
    List<List<String>> mediaTags = const [],
  }) => _submit(
    kind: EventKind.forumComment,
    channelId: channelId,
    parentEventId: parentEventId,
    content: content,
    mentionPubkeys: mentionPubkeys,
    mediaTags: mediaTags,
  );

  Future<void> _submit({
    required int kind,
    required String channelId,
    required String content,
    String? parentEventId,
    required List<String> mentionPubkeys,
    required List<List<String>> mediaTags,
    void Function()? onShown,
  }) async {
    final currentConfig = _container.read(relayConfigProvider);
    if (currentConfig.baseUrl != _relayUrl ||
        currentConfig.nsec != _nsec ||
        outgoingAuthorPubkey(currentConfig) != _relay.pubkey) {
      throw StateError(
        'Forum delivery cancelled because the active community changed',
      );
    }

    final selfPubkey = _relay.pubkey?.toLowerCase();
    final seen = <String>{?selfPubkey};
    final normalizedMentions = [
      for (final pk in mentionPubkeys)
        if (seen.add(pk.toLowerCase())) pk,
    ];

    // Started before the row exists, so its acceptance is never missed.
    _container.read(forumAcceptedSendsProvider(channelId));
    ChannelMessagesNotifier messages() =>
        _container.read(channelMessagesProvider(channelId).notifier);
    NostrEvent? shown;
    try {
      await _relay.submit(
        kind: kind,
        content: content,
        tags: [
          ['h', channelId],
          if (parentEventId != null) ['e', parentEventId, '', 'reply'],
          for (final pk in normalizedMentions) ['p', pk],
          ...mediaTags,
          ...buildCustomEmojiTags(content, _customEmoji),
        ],
        onSigned: (event) {
          shown = event;
          // Persisted, so a send that fails or is cut off survives a restart.
          messages().addLocalMessage(event, persist: true);
          onShown?.call();
        },
      );
    } catch (error) {
      final event = shown;
      if (event == null) rethrow;
      debugPrint('[ForumEventDelivery] publish failed for ${event.id}: $error');
      _container
          .read(failedLocalMessagesProvider(channelId).notifier)
          .markFailed(event.id);
      return;
    }
    // [forumAcceptedSendsProvider] keeps the row until its list refetches.
    if (shown case final event?) messages().completeLocalMessage(event.id);
  }
}

/// Deletes a forum post or reply and invalidates relevant caches.
Future<void> deleteForumEvent(
  WidgetRef ref, {
  required String channelId,
  required String eventId,
  String? rootEventId,
}) async {
  final actions = ref.read(channelActionsProvider);
  await actions.deleteMessage(channelId: channelId, eventId: eventId);
  ref.invalidate(forumPostsProvider(channelId));
  if (rootEventId != null) {
    ref.invalidate(
      forumThreadProvider((channelId: channelId, eventId: rootEventId)),
    );
  }
}
