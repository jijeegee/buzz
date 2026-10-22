part of 'channel_management_provider.dart';

/// Builds the relay tags for setting the archived state of [channelId].
List<List<String>> buildSetChannelArchivedTags(
  String channelId, {
  required bool archived,
}) => [
  ['h', channelId],
  ['archived', archived.toString()],
];

/// Builds the relay tags for deleting [channelId].
List<List<String>> buildDeleteChannelTags(String channelId) => [
  ['h', channelId],
];

/// Most people a new chat can start with, besides me.
const groupChatCreateLimit = 100;

/// Builds the one kind:9007 that creates a chat of people: private stream,
/// `group_chat` flag, optional shared name, and the starting people.
List<List<String>> buildCreateGroupChatTags({
  required String channelId,
  required Iterable<String> pubkeys,
  String? name,
  String? currentPubkey,
}) {
  final me = currentPubkey?.toLowerCase();
  final people = {
    for (final pubkey in pubkeys)
      if (pubkey.trim().isNotEmpty && pubkey.trim().toLowerCase() != me)
        pubkey.trim().toLowerCase(),
  };
  if (people.length > groupChatCreateLimit) {
    throw ArgumentError.value(
      people.length,
      'pubkeys',
      'at most $groupChatCreateLimit people',
    );
  }
  final trimmedName = name?.trim() ?? '';
  return [
    ['h', channelId],
    ['group_chat'],
    ['visibility', 'private'],
    ['channel_type', 'stream'],
    if (trimmedName.isNotEmpty) ['name', trimmedName],
    for (final pubkey in people) ['p', pubkey],
  ];
}

/// "invite-sent:N" in a create reply: how many people got an invitation.
int? inviteSentCountFromReply(String reply) {
  final match = RegExp(r'invite-sent:\s*(\d+)').firstMatch(reply);
  return match == null ? null : int.tryParse(match.group(1)!);
}

/// A new chat and how many of the people picked were sent an invitation
/// instead of joining.
@immutable
class CreateGroupChatResult {
  const CreateGroupChatResult({
    required this.channel,
    required this.invitedCount,
    required this.pickedCount,
    this.rateLimited = false,
  });

  final Channel channel;
  final int invitedCount;
  final int pickedCount;

  /// Some people were skipped: too many invitations today.
  final bool rateLimited;

  /// The note to show after creating, or null when everyone joined:
  /// "Invitations sent" when nobody joined, else "Invitation sent to N".
  String? get notice {
    if (invitedCount <= 0) {
      return rateLimited ? tooManyInvitationsMessage : null;
    }
    if (invitedCount >= pickedCount) return 'Invitations sent';
    return 'Invitation sent to $invitedCount';
  }
}

class ChannelActions {
  final Ref _ref;
  final RelaySessionNotifier _session;
  final SignedEventRelay _signedEventRelay;
  final String? _currentPubkey;
  final bool Function()? _isCommunityValid;

  ChannelActions({
    required Ref ref,
    required RelaySessionNotifier session,
    required SignedEventRelay signedEventRelay,
    required String? currentPubkey,
    bool Function()? isCommunityValid,
  }) : _ref = ref,
       _session = session,
       _signedEventRelay = signedEventRelay,
       _currentPubkey = currentPubkey,
       _isCommunityValid = isCommunityValid;

  Future<Channel> createChannel({
    String? channelId,
    required String name,
    required String channelType,
    required String visibility,
    String? description,
    int? ttlSeconds,
  }) async {
    final resolvedChannelId = channelId ?? _newUuidV4();
    final tags = buildCreateChannelTags(
      channelId: resolvedChannelId,
      name: name,
      channelType: channelType,
      visibility: visibility,
      description: description,
      ttlSeconds: ttlSeconds,
    );
    _ensureCommunityValid();
    await _signedEventRelay.submit(kind: 9007, content: '', tags: tags);
    _ensureCommunityValid();
    return _refreshChannelsAndRead(resolvedChannelId);
  }

  /// Creates a chat with [pubkeys] (up to [groupChatCreateLimit]) in one
  /// kind:9007. The relay adds each person or sends them an invitation; the
  /// member list that comes back is what counts.
  Future<CreateGroupChatResult> createGroupChat({
    required List<String> pubkeys,
    String? name,
  }) async {
    final channelId = _newUuidV4();
    final tags = buildCreateGroupChatTags(
      channelId: channelId,
      pubkeys: pubkeys,
      name: name,
      currentPubkey: _currentPubkey,
    );
    final picked = [
      for (final tag in tags)
        if (tag.first == 'p') tag[1],
    ];
    _ensureCommunityValid();
    final reply = await _signedEventRelay.submit(
      kind: 9007,
      content: '',
      tags: tags,
    );
    _ensureCommunityValid();
    final channel = await _refreshChannelsAndRead(channelId);
    final members = channel.participantPubkeys
        .map((pubkey) => pubkey.toLowerCase())
        .toSet();
    final invitedCount = members.isEmpty
        ? inviteSentCountFromReply(reply.content) ?? 0
        : picked.where((pubkey) => !members.contains(pubkey)).length;
    return CreateGroupChatResult(
      channel: channel,
      invitedCount: invitedCount,
      pickedCount: picked.length,
      rateLimited: reply.content.contains('rate-limited'),
    );
  }

  /// Open (or create) the 1:1 DM with one other person, or my own notes
  /// when [pubkeys] is only me (or empty). More people make a chat with
  /// [createGroupChat] instead.
  ///
  /// This submits a kind:41010 command event; the relay responds with an OK
  /// message whose content carries `response:{...}` containing the new
  /// `channel_id`.
  Future<Channel> openDm({required List<String> pubkeys}) async {
    final others = {
      for (final pubkey in pubkeys)
        if (pubkey.toLowerCase() != _currentPubkey?.toLowerCase())
          pubkey.toLowerCase(),
    };
    if (others.length > 1) {
      throw ArgumentError.value(
        pubkeys,
        'pubkeys',
        'a DM has one other person; start a chat for more',
      );
    }
    _ensureCommunityValid();
    final result = await _signedEventRelay.submit(
      kind: 41010,
      content: '',
      tags: pubkeys.map((pk) => ['p', pk]).toList(),
    );
    _ensureCommunityValid();
    final response = parseCommandResponse(result.content);
    final channelId = response?['channel_id'] as String?;
    if (channelId == null || channelId.isEmpty) {
      throw Exception('Relay did not return a DM channel id');
    }
    return _refreshChannelsAndRead(channelId);
  }

  /// Adds [pubkeys] with kind:9000. People the relay sent an invitation to
  /// instead (their "who can add me" setting) come back in
  /// [AddMembersResult.invited]; refusals throw [AddMembersException].
  Future<AddMembersResult> addMembers({
    required String channelId,
    required List<String> pubkeys,
    String role = 'member',
  }) async {
    final normalizedRole = role.trim();
    if (normalizedRole.isEmpty) {
      throw ArgumentError.value(role, 'role', 'must not be empty');
    }
    final normalizedPubkeys = {
      for (final pubkey in pubkeys)
        if (pubkey.trim().isNotEmpty) pubkey.trim().toLowerCase(),
    };
    _ensureCommunityValid();
    // Per-pubkey failures are collected rather than thrown on the spot: one
    // relay rejection must not skip the remaining adds or the invalidation
    // below, which would leave the members list stale for the adds that landed.
    final failures = <String, String>{};
    final invited = <String>[];
    for (final pubkey in normalizedPubkeys) {
      // Outside the catch: a community switch mid-loop must abort the whole
      // add, not be recorded as this pubkey's rejection.
      _ensureCommunityValid();
      try {
        await _signedEventRelay.submit(
          kind: 9000,
          content: '',
          tags: [
            ['h', channelId],
            ['p', pubkey],
            ['role', normalizedRole],
          ],
        );
      } catch (error) {
        final message = _relayErrorMessage(error);
        switch (classifyAddMemberReply(message)) {
          case AddMemberReply.invited:
            invited.add(pubkey);
          case AddMemberReply.rateLimited:
            failures[pubkey] = tooManyInvitationsMessage;
          case AddMemberReply.other:
            failures[pubkey] = message;
        }
      }
    }
    _ensureCommunityValid();
    _ref.invalidate(channelMembersProvider(channelId));
    _ref.invalidate(channelBotPubkeysProvider(channelId));
    if (failures.isNotEmpty) {
      throw AddMembersException(failures, invited: invited);
    }
    return AddMembersResult(invited: invited);
  }

  void _ensureCommunityValid() {
    if (_isCommunityValid?.call() == false) {
      throw StateError(
        'Group action cancelled because the active community changed',
      );
    }
  }

  Future<void> joinChannel(String channelId) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: 9021,
      content: '',
      tags: [
        ['h', channelId],
      ],
    );
    _ensureCommunityValid();
    await _refreshChannelState(channelId);
  }

  Future<void> leaveChannel(String channelId) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: 9022,
      content: '',
      tags: [
        ['h', channelId],
      ],
    );
    _ensureCommunityValid();
    await _refreshChannelState(channelId);
  }

  /// Updates the user-editable channel metadata and refreshes cached details.
  /// [goalsEnabled] toggles the per-room goals switch and may be sent alone.
  /// [picture] sets the group photo URL; an empty string removes it.
  Future<void> updateChannel({
    required String channelId,
    String? name,
    String? description,
    bool? goalsEnabled,
    String? visibility,
    String? picture,
  }) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: 9002,
      content: '',
      tags: buildUpdateChannelTags(
        channelId: channelId,
        name: name,
        description: description,
        goalsEnabled: goalsEnabled,
        visibility: visibility,
        picture: picture,
      ),
    );
    _ensureCommunityValid();
    await _refreshChannelState(channelId);
  }

  /// Archives the channel and refreshes its cached state.
  Future<void> archiveChannel(String channelId) =>
      _setChannelArchived(channelId, archived: true);

  /// Unarchives the channel and refreshes its cached state.
  Future<void> unarchiveChannel(String channelId) =>
      _setChannelArchived(channelId, archived: false);

  Future<void> _setChannelArchived(
    String channelId, {
    required bool archived,
  }) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: 9002,
      content: '',
      tags: buildSetChannelArchivedTags(channelId, archived: archived),
    );
    _ensureCommunityValid();
    await _refreshChannelState(channelId);
  }

  /// Deletes the channel and refreshes its cached state.
  Future<void> deleteChannel(String channelId) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: 9008,
      content: '',
      tags: buildDeleteChannelTags(channelId),
    );
    _ensureCommunityValid();
    await _refreshChannelState(channelId);
  }

  Future<void> setCanvas({
    required String channelId,
    required String content,
  }) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: 40100,
      content: content,
      tags: [
        ['h', channelId],
      ],
    );
    _ensureCommunityValid();
    _ref.invalidate(channelCanvasProvider(channelId));
  }

  /// User search via NIP-50 over kind:0 profile events.
  Future<List<DirectoryUser>> searchUsers(String query, {int limit = 8}) async {
    final trimmed = query.trim();
    if (trimmed.isEmpty) return const [];

    final events = await _session.queryRelay([
      NostrFilters.searchUsers(trimmed, limit: limit),
    ]);
    return directoryUsersFromProfileEvents(events)
        .where(
          (user) =>
              _currentPubkey == null ||
              user.pubkey.toLowerCase() != _currentPubkey,
        )
        .toList();
  }

  Future<Channel> _refreshChannelsAndRead(String channelId) async {
    _ensureCommunityValid();
    await _ref.read(channelsProvider.notifier).refresh();
    _ensureCommunityValid();
    final channels = await _ref.read(channelsProvider.future);
    _ensureCommunityValid();
    return channels.firstWhere(
      (channel) => channel.id == channelId,
      orElse: () => throw Exception('Group was created but is not visible yet'),
    );
  }

  Future<void> _refreshChannelState(String channelId) async {
    _ensureCommunityValid();
    await _ref.read(channelsProvider.notifier).refresh();
    _ensureCommunityValid();
    _ref.invalidate(channelDetailsProvider(channelId));
    _ref.invalidate(channelMembersProvider(channelId));
    _ref.invalidate(channelBotPubkeysProvider(channelId));
    _ref.invalidate(channelCanvasProvider(channelId));
  }

  String _newUuidV4() {
    final bytes = List<int>.generate(16, (_) => _random.nextInt(256));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    final hex = bytes
        .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
        .join();
    return '${hex.substring(0, 8)}-'
        '${hex.substring(8, 12)}-'
        '${hex.substring(12, 16)}-'
        '${hex.substring(16, 20)}-'
        '${hex.substring(20, 32)}';
  }

  Future<void> changeMemberRole({
    required String channelId,
    required String pubkey,
    required String role,
  }) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: 9000,
      content: '',
      tags: [
        ['h', channelId],
        ['p', pubkey.toLowerCase()],
        ['role', role],
      ],
    );
    _ensureCommunityValid();
    _ref.invalidate(channelMembersProvider(channelId));
    _ref.invalidate(channelBotPubkeysProvider(channelId));
  }

  Future<void> removeMember({
    required String channelId,
    required String pubkey,
  }) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: 9001,
      content: '',
      tags: [
        ['h', channelId],
        ['p', pubkey.toLowerCase()],
      ],
    );
    _ensureCommunityValid();
    _ref.invalidate(channelMembersProvider(channelId));
    _ref.invalidate(channelBotPubkeysProvider(channelId));
  }

  Future<void> addReaction(String eventId, String emoji) async {
    final shortcode = normalizeShortcode(emoji);
    final emojiUrl = reactionEmojiUrl(
      emoji,
      _ref.read(customEmojiListProvider),
    );
    await _signedEventRelay.submit(
      kind: EventKind.reaction,
      content: emoji,
      tags: [
        ['e', eventId],
        if (shortcode != null && emojiUrl != null)
          ['emoji', shortcode, emojiUrl],
      ],
    );
  }

  Future<void> removeReaction(String reactionEventId, String emoji) async {
    await _signedEventRelay.submit(
      kind: EventKind.deletion,
      content: '',
      tags: [
        ['e', reactionEventId],
      ],
    );
  }

  Future<void> editMessage({
    required String channelId,
    required String eventId,
    required String content,
    List<List<String>> mediaTags = const [],
  }) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: EventKind.streamMessageEdit,
      content: content,
      tags: [
        ['h', channelId],
        ['e', eventId],
        ...mediaTags,
      ],
    );
    _ensureCommunityValid();
  }

  Future<void> deleteMessage({
    required String channelId,
    required String eventId,
  }) async {
    _ensureCommunityValid();
    await _signedEventRelay.submit(
      kind: EventKind.deletion,
      content: '',
      tags: buildDeleteMessageTags(channelId: channelId, eventId: eventId),
    );
    _ensureCommunityValid();
  }

  static final Random _random = Random.secure();
}

final channelActionsProvider = Provider<ChannelActions>((ref) {
  final relayConfig = ref.watch(relayConfigProvider);
  final currentPubkey = ref.watch(currentPubkeyProvider);
  final session = ref.read(relaySessionProvider.notifier);
  return ChannelActions(
    ref: ref,
    session: session,
    signedEventRelay: SignedEventRelay.forConfig(
      session: session,
      config: relayConfig,
    ),
    currentPubkey: currentPubkey,
    isCommunityValid: () {
      final currentConfig = ref.read(relayConfigProvider);
      return currentConfig.baseUrl == relayConfig.baseUrl &&
          currentConfig.nsec == relayConfig.nsec &&
          outgoingAuthorPubkey(currentConfig) ==
              outgoingAuthorPubkey(relayConfig);
    },
  );
});
