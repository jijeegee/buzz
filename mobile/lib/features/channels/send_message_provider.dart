import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../channels/channel_management_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import 'channel.dart';
import 'channel_messages_provider.dart';
import 'failed_local_messages_provider.dart';
import 'message_mention_pubkeys.dart';
import 'message_quote.dart';
import 'local_message_send_animation_provider.dart';
import 'pending_local_messages_provider.dart';

/// A failed message older than this is re-signed with a fresh timestamp on
/// retry; the relay refuses events more than 15 minutes off its clock.
const _retryReuseWindowSeconds = 10 * 60;

/// Sends messages by signing an event with the user's nsec and publishing it
/// over the relay's NIP-42-authenticated WebSocket session.
class SendMessage {
  final SignedEventRelay _signedEventRelay;
  final Future<List<ChannelMember>> Function(String channelId) _fetchMembers;
  final List<ChannelMember>? Function(String channelId) _readCachedMembers;
  final Map<String, UserProfile> Function() _readUserCache;
  final void Function(String channelId, NostrEvent event) _addLocalMessage;
  final void Function(String channelId, NostrEvent event)
  _addPersistedLocalMessage;
  final void Function(String channelId, String eventId)
  _markLocalMessageForAnimation;
  final void Function(String channelId, String eventId) _completeLocalMessage;
  final void Function(String channelId, String eventId) _removeLocalMessage;
  final void Function(String channelId, String eventId) _markLocalMessageFailed;
  final void Function(String channelId, String eventId)
  _clearLocalMessageFailed;
  final NostrEvent? Function(String channelId, String eventId)
  _readLocalMessage;
  final bool Function()? _isDeliveryValid;

  SendMessage({
    required SignedEventRelay signedEventRelay,
    required Future<List<ChannelMember>> Function(String channelId)
    fetchMembers,
    List<ChannelMember>? Function(String channelId)? readCachedMembers,
    required Map<String, UserProfile> Function() readUserCache,
    required void Function(String channelId, NostrEvent event) addLocalMessage,
    void Function(String channelId, NostrEvent event)? addPersistedLocalMessage,
    void Function(String channelId, String eventId)?
    markLocalMessageForAnimation,
    required void Function(String channelId, String eventId)
    completeLocalMessage,
    required void Function(String channelId, String eventId) removeLocalMessage,
    void Function(String channelId, String eventId)? markLocalMessageFailed,
    void Function(String channelId, String eventId)? clearLocalMessageFailed,
    NostrEvent? Function(String channelId, String eventId)? readLocalMessage,
    bool Function()? isDeliveryValid,
  }) : _signedEventRelay = signedEventRelay,
       _fetchMembers = fetchMembers,
       _readCachedMembers = readCachedMembers ?? ((_) => null),
       _readUserCache = readUserCache,
       _addLocalMessage = addLocalMessage,
       _addPersistedLocalMessage = addPersistedLocalMessage ?? addLocalMessage,
       _markLocalMessageForAnimation =
           markLocalMessageForAnimation ?? ((_, _) {}),
       _completeLocalMessage = completeLocalMessage,
       _removeLocalMessage = removeLocalMessage,
       _markLocalMessageFailed = markLocalMessageFailed ?? ((_, _) {}),
       _clearLocalMessageFailed = clearLocalMessageFailed ?? ((_, _) {}),
       _readLocalMessage = readLocalMessage ?? ((_, _) => null),
       _isDeliveryValid = isDeliveryValid;

  /// Send a text message to a channel.
  ///
  /// For thread replies, pass [parentEventId] and optionally [rootEventId].
  /// If [rootEventId] is null it defaults to [parentEventId] (direct reply to
  /// thread head). Tags are built to match the desktop's `buildReplyTags`
  /// convention with `root` / `reply` markers. Pass [mediaTags] to append
  /// relay-validated `imeta` tags and NIP-30 `emoji` tags.
  ///
  /// Pass [quote] to add exactly one NIP-18 `["q", id, "", author]` tag. A
  /// quote is a reference only: it adds no `e` or `p` tags, so it never
  /// changes where the message lands or who is notified.
  ///
  /// [extraTags] are appended as given (e.g. a thread close marker).
  ///
  /// With [keepFailedMessage], a publish that fails after the message was
  /// shown marks the row failed (for retry or delete) and completes normally,
  /// and the row is kept on the device until the relay has it.
  /// Otherwise the row is rolled back and the error rethrown. Failures before
  /// the row is shown always throw.
  Future<void> call({
    required String channelId,
    required String content,
    String? parentEventId,
    String? rootEventId,
    List<String>? mentionPubkeys,
    Channel? channel,
    List<List<String>> mediaTags = const [],
    QuoteTarget? quote,
    List<List<String>> extraTags = const [],
    bool keepFailedMessage = false,
  }) async {
    _ensureDeliveryValid();
    // Validate before any network work so a malformed quote fails fast.
    final quoteTag = quote == null
        ? null
        : buildQuoteTag(
            eventId: quote.eventId,
            authorPubkey: quote.authorPubkey,
          );
    // Use explicitly passed pubkeys, or resolve @mentions against
    // channel members to avoid matching the wrong user.
    final explicitMentions =
        mentionPubkeys ?? await _resolveMentions(content, channelId);
    final authorPubkey = _signedEventRelay.pubkey;
    final dmRecipientPubkeys = channel?.isDm == true
        ? await _fetchDmRecipientPubkeys(channelId, channel!, authorPubkey)
        : null;
    final resolvedMentions = dmRecipientPubkeys != null
        ? messageMentionPubkeys(
            channel: channel!,
            senderPubkey: authorPubkey,
            explicitMentions: explicitMentions,
            dmRecipientPubkeys: dmRecipientPubkeys,
          )
        : explicitMentions;

    // Normalize mentions: lowercase, deduplicate, exclude self (matching
    // the desktop's normalizeMentionPubkeys).
    final selfLower = authorPubkey?.toLowerCase();
    final seenMentions = <String>{?selfLower};
    final normalizedMentions = <String>[
      for (final pk in resolvedMentions)
        if (seenMentions.add(pk.toLowerCase())) pk,
    ];

    final tags = <List<String>>[
      ['h', channelId],
      if (parentEventId != null) ..._buildReplyTags(parentEventId, rootEventId),
      for (final pk in normalizedMentions) ['p', pk],
      for (final tag in mediaTags)
        if (quoteTag == null || tag.isEmpty || tag.first != quoteTagName) tag,
      ?quoteTag,
      ...extraTags,
    ];

    _ensureDeliveryValid();
    await _submitLocal(
      channelId: channelId,
      kind: EventKind.streamMessage,
      content: content,
      tags: tags,
      keepFailedMessage: keepFailedMessage,
      onSigned: (event) {
        _markLocalMessageForAnimation(channelId, event.id);
        (keepFailedMessage ? _addPersistedLocalMessage : _addLocalMessage)(
          channelId,
          event,
        );
      },
    );
  }

  /// Publishes the failed local message [eventId] again.
  ///
  /// The row returns to pending first. A recent message is rebuilt with its
  /// original timestamp, which reproduces the same event id: if an earlier
  /// attempt did land, the relay dedupes it and its echo replaces the row. An
  /// older one is re-signed now: the failed row is removed and the new one
  /// is added at the bottom of the timeline. A new publish failure marks the
  /// row failed again; nothing is thrown for it.
  Future<void> retry({
    required String channelId,
    required String eventId,
  }) async {
    _ensureDeliveryValid();
    final failed = _readLocalMessage(channelId, eventId);
    if (failed == null) return;
    final author = _signedEventRelay.pubkey?.toLowerCase();
    if (author == null || failed.pubkey.toLowerCase() != author) {
      throw StateError('Message was written by another account');
    }
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    final reuse = (now - failed.createdAt).abs() < _retryReuseWindowSeconds;
    await _submitLocal(
      channelId: channelId,
      kind: failed.kind,
      content: failed.content,
      tags: [
        for (final tag in failed.tags) [...tag],
      ],
      createdAt: reuse ? failed.createdAt : null,
      keepFailedMessage: true,
      onSigned: (event) {
        if (event.id == failed.id) {
          _clearLocalMessageFailed(channelId, failed.id);
          return;
        }
        _removeLocalMessage(channelId, failed.id);
        _addPersistedLocalMessage(channelId, event);
      },
    );
  }

  /// Drops the failed local message [eventId] without sending it.
  void discard({required String channelId, required String eventId}) =>
      _removeLocalMessage(channelId, eventId);

  Future<void> _submitLocal({
    required String channelId,
    required int kind,
    required String content,
    required List<List<String>> tags,
    required bool keepFailedMessage,
    required void Function(NostrEvent event) onSigned,
    int? createdAt,
  }) async {
    NostrEvent? localMessage;
    try {
      await _signedEventRelay.submit(
        kind: kind,
        content: content,
        tags: tags,
        createdAt: createdAt,
        onSigned: (event) {
          localMessage = event;
          onSigned(event);
        },
      );
      final event = localMessage;
      if (event != null) _completeLocalMessage(channelId, event.id);
    } catch (error) {
      final event = localMessage;
      if (event == null) rethrow;
      if (keepFailedMessage) {
        debugPrint('[SendMessage] publish failed for ${event.id}: $error');
        _markLocalMessageFailed(channelId, event.id);
        return;
      }
      _removeLocalMessage(channelId, event.id);
      rethrow;
    }
  }

  /// Current members of [channelId]: the loaded snapshot when there is one,
  /// so a send never waits on a relay round trip it doesn't need.
  Future<List<ChannelMember>> _members(String channelId) async {
    List<ChannelMember>? cached;
    try {
      cached = _readCachedMembers(channelId);
    } catch (_) {
      // An errored snapshot is no snapshot; fetch below.
    }
    if (cached != null && cached.isNotEmpty) return cached;
    return _fetchMembers(channelId);
  }

  /// Resolve every identity that is actually a current member of this DM.
  ///
  /// Membership is authoritative for delivery. The channel metadata's `p`
  /// tags can lag membership changes, so they are only used when the membership
  /// snapshot is unavailable.
  Future<Set<String>> _fetchDmRecipientPubkeys(
    String channelId,
    Channel channel,
    String? authorPubkey,
  ) async {
    List<ChannelMember>? members;
    try {
      members = await _members(channelId);
    } catch (_) {
      // Fall back to metadata below so an unavailable membership query does
      // not block ordinary DM sends.
    }

    final author = authorPubkey?.toLowerCase();
    final participants = members != null && members.isNotEmpty
        ? members.map((member) => member.pubkey)
        : channel.participantPubkeys;
    return {
      for (final participant in participants)
        if (participant.trim().isNotEmpty &&
            participant.toLowerCase() != author)
          participant.toLowerCase(),
    };
  }

  void _ensureDeliveryValid() {
    if (_isDeliveryValid?.call() == false) {
      throw StateError(
        'Message delivery cancelled because the active community changed',
      );
    }
  }

  /// Resolve @mentions to pubkeys, scoped to channel members.
  ///
  /// Fetches channel members from the relay and matches @names only
  /// against members of that channel. Falls back to the full user cache
  /// if the member fetch fails.
  Future<List<String>> _resolveMentions(
    String content,
    String channelId,
  ) async {
    final mentionPattern = RegExp(r'@(\w+)');
    final matches = mentionPattern.allMatches(content);
    if (matches.isEmpty) return const [];

    // Try to get channel member pubkeys for scoped resolution.
    Set<String>? memberPubkeys;
    try {
      final members = await _members(channelId);
      memberPubkeys = {for (final m in members) m.pubkey.toLowerCase()};
    } catch (_) {
      // Non-fatal — fall through to unscoped cache lookup.
    }

    final cache = _readUserCache();
    final pubkeys = <String>{};

    for (final match in matches) {
      final name = match.group(1)?.toLowerCase();
      if (name == null || name.isEmpty) continue;

      for (final profile in cache.values) {
        final displayName = profile.displayName?.toLowerCase();
        if (displayName == null) continue;

        // Match against full display name or first word.
        final firstName = displayName.split(RegExp(r'\s+')).first;
        if (displayName != name && firstName != name) continue;

        // If we have channel members, only match members of this channel.
        if (memberPubkeys != null &&
            !memberPubkeys.contains(profile.pubkey.toLowerCase())) {
          continue;
        }

        pubkeys.add(profile.pubkey);
        break;
      }
    }

    return pubkeys.toList();
  }

  /// Build `e`-tags for a thread reply, matching the desktop convention:
  /// - Direct reply to thread head: `["e", id, "", "reply"]`
  /// - Nested reply: `["e", rootId, "", "root"]` + `["e", parentId, "", "reply"]`
  static List<List<String>> _buildReplyTags(
    String parentEventId,
    String? rootEventId,
  ) {
    final root = rootEventId ?? parentEventId;
    if (parentEventId == root) {
      return [
        ['e', root, '', 'reply'],
      ];
    }
    return [
      ['e', root, '', 'root'],
      ['e', parentEventId, '', 'reply'],
    ];
  }
}

final sendMessageProvider = Provider<SendMessage>((ref) {
  final config = ref.watch(relayConfigProvider);
  return SendMessage(
    signedEventRelay: SignedEventRelay.forConfig(
      session: ref.read(relaySessionProvider.notifier),
      config: config,
    ),
    fetchMembers: (channelId) =>
        ref.read(channelMembersProvider(channelId).future),
    // Only a provider something already watches (an open conversation) has
    // a value here; reading it never starts a fetch the send would wait on.
    readCachedMembers: (channelId) {
      final provider = channelMembersProvider(channelId);
      return ref.exists(provider) ? ref.read(provider).value : null;
    },
    readUserCache: () => ref.read(userCacheProvider),
    addLocalMessage: (channelId, event) => ref
        .read(channelMessagesProvider(channelId).notifier)
        .addLocalMessage(event),
    addPersistedLocalMessage: (channelId, event) => ref
        .read(channelMessagesProvider(channelId).notifier)
        .addLocalMessage(event, persist: true),
    markLocalMessageForAnimation: (channelId, eventId) => ref
        .read(localMessageSendAnimationProvider(channelId).notifier)
        .mark(eventId),
    completeLocalMessage: (channelId, eventId) => ref
        .read(channelMessagesProvider(channelId).notifier)
        .completeLocalMessage(eventId),
    removeLocalMessage: (channelId, eventId) => ref
        .read(channelMessagesProvider(channelId).notifier)
        .removeLocalMessage(eventId),
    markLocalMessageFailed: (channelId, eventId) => ref
        .read(failedLocalMessagesProvider(channelId).notifier)
        .markFailed(eventId),
    clearLocalMessageFailed: (channelId, eventId) => ref
        .read(failedLocalMessagesProvider(channelId).notifier)
        .clear(eventId),
    readLocalMessage: (channelId, eventId) =>
        ref.read(pendingLocalMessagesProvider(channelId))[eventId],
    isDeliveryValid: () {
      final currentConfig = ref.read(relayConfigProvider);
      return currentConfig.baseUrl == config.baseUrl &&
          currentConfig.nsec == config.nsec &&
          outgoingAuthorPubkey(currentConfig) == outgoingAuthorPubkey(config);
    },
  );
});
