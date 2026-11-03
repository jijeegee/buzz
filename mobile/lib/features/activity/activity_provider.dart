import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_state/read_state_time.dart';
import '../../shared/relay/me_live_bundle.dart';
import '../../shared/relay/relay.dart';
import '../../shared/relay/relay_connection_epoch.dart';
import '../channels/channel.dart';
import '../channels/channel_live_events.dart';
import '../channels/channel_management_provider.dart';
import '../channels/channels_provider.dart';
import '../channels/room_live_router.dart';
import '../people/blocked_users_provider.dart';
import 'dm_resurface.dart';
import 'feed_item.dart';
import 'inbox_item.dart';
import 'inbox_summary.dart';
import 'inbox_summary_provider.dart';

typedef DmResurfaceAction = Future<String> Function(List<String> pubkeys);

/// After a reconnect, how long the hidden-DM gap read waits again for the
/// quiet rooms' chunks when the first wait ([roomCoverageWaitLimit]) timed
/// out.
const hiddenDmLateReadyLimit = Duration(seconds: 30);

final dmResurfaceActionProvider = Provider<DmResurfaceAction>(
  (ref) => (pubkeys) async {
    final channel = await ref
        .read(channelActionsProvider)
        .openDm(pubkeys: pubkeys);
    return channel.id;
  },
);

/// Builds the Activity inbox feed over the relay websocket.
///
/// Sources mirror desktop's Home inbox (`useHomeFeedQuery` + `get_feed`):
/// - mentions of me on user-visible channel kinds (also yields thread
///   replies, which the thread filter classifies from NIP-10 tags)
/// - workflow approvals / needs-action events addressed to me
/// - agent job lifecycle events addressed to me (kinds 43001-43006)
/// - recent DM messages from others (desktop surfaces DMs through p-tags;
///   mobile queries DM channels directly so untagged DM sends still appear)
class ActivityNotifier extends AsyncNotifier<HomeFeedResponse> {
  static const _addressedKinds = [
    1,
    9,
    40002,
    43001,
    43002,
    43003,
    43004,
    43005,
    43006,
    45001,
    45003,
    46010,
    46011,
    46012,
  ];

  void Function()? _unsubscribeAddressed;
  void Function()? _unsubscribeVisibleDms;
  void Function()? _unsubscribeQuiet;
  // When the connection dropped (after the first connect) and the room
  // router's ready tick then: the next connection reads the hidden DMs' gap.
  int? _droppedAt;
  int _dropReadyTick = 0;
  bool _connectedOnce = false;
  Timer? _liveRefreshTimer;
  Future<void>? _refreshInFlight;
  int? _refreshGeneration;
  bool _refreshQueued = false;
  int _subscriptionGeneration = 0;

  /// The generation whose build() is still fetching. A live event meanwhile
  /// only sets [_pendingDuringBuild]: a refresh landing first would be
  /// overwritten by build's older result. One refresh follows the build.
  int? _buildingGeneration;
  bool _pendingDuringBuild = false;
  // Per-hidden-channel resurface coalescing. Presence of a key means an attempt
  // is in flight; the entry records the owning subscription generation and
  // whether a follower event arrived while it was running, so a failed reopen
  // re-runs instead of dropping the follower. The entry is generation-owned so
  // a follower on a rebuilt subscription starts its own attempt instead of
  // coalescing into a suspended old-generation attempt that will never resume.
  final Map<String, _PendingResurface> _pendingDmResurfaceRetry = {};
  String? _dmResurfaceScope;

  @override
  Future<HomeFeedResponse> build() async {
    ref.watch(relayConfigProvider);
    // Once per usable connection, not per status step: each step (and each
    // reconnect attempt) would fetch the feed and resubscribe again. A
    // reconnect refetches, so nothing missed while away is lost.
    ref.watch(relayConnectionEpochProvider);
    // React to the DM channel set (loading → data, membership changes) so a
    // cold start where channels resolve after the first fetch still surfaces
    // DMs without a manual refresh.
    // The saved list drawn at launch is not the relay's: it counts as none.
    final channelsNotifier = ref.read(channelsProvider.notifier);
    ref.watch(
      channelsProvider.select(
        (v) => channelsNotifier.showsSavedList ? null : _dmChannelKey(v),
      ),
    );

    final generation = ++_subscriptionGeneration;
    final currentPubkey = ref.read(myPubkeyProvider)?.toLowerCase();
    final currentScope =
        '${ref.read(relayConfigProvider).baseUrl}\u0000$currentPubkey';
    if (_dmResurfaceScope != currentScope) {
      _dmResurfaceScope = currentScope;
      _pendingDmResurfaceRetry.clear();
      _droppedAt = null;
      _connectedOnce = false;
    }
    _clearLiveSubscriptions();
    ref.onDispose(() {
      _subscriptionGeneration += 1;
      _clearLiveSubscriptions();
    });
    _watchDrops();
    // Hidden DMs ride the room chunks as quiet rooms (no REQ of their own):
    // the router hands their events here, never to the all-rooms consumers.
    _unsubscribeQuiet = ref
        .read(roomLiveRouterProvider)
        .listenQuiet((event, _) => _handleHiddenDmLiveEvent(event, generation));

    final connected =
        ref.read(relaySessionProvider).status == SessionStatus.connected;
    final droppedAt = connected ? _droppedAt : null;
    if (droppedAt != null) _droppedAt = null;
    final lateDms = connected
        ? _listenVisibleDms(generation)
        : const <String>[];
    _buildingGeneration = generation;
    _pendingDuringBuild = false;
    final HomeFeedResponse response;
    try {
      // Listen before fetching, so nothing addressed to me in between is lost.
      if (connected) await _subscribeAddressed(generation);
      response = await _fetch();
    } finally {
      if (_buildingGeneration == generation) _buildingGeneration = null;
    }
    if (connected && generation == _subscriptionGeneration) {
      if (lateDms.isNotEmpty) {
        unawaited(_refreshOnDmCoverage(generation, lateDms));
      }
      if (droppedAt != null) {
        unawaited(_readHiddenDmGap(generation, droppedAt, _dropReadyTick));
      }
      if (_pendingDuringBuild) {
        _pendingDuringBuild = false;
        _scheduleLiveRefresh(generation);
      }
    }
    return response;
  }

  /// Records when a usable connection drops (not before the first connect).
  void _watchDrops() {
    ref.listen<bool>(
      relaySessionProvider.select(
        (session) => session.status == SessionStatus.connected,
      ),
      (_, connected) {
        if (connected) {
          _connectedOnce = true;
        } else if (_connectedOnce && _droppedAt == null) {
          _droppedAt = DateTime.now().millisecondsSinceEpoch ~/ 1000;
          _dropReadyTick = ref.read(roomLiveRouterProvider).readyTick;
        }
      },
      fireImmediately: true,
    );
  }

  /// Hidden DMs' messages sent while the connection was down: read once their
  /// chunks are live on the new connection (a bounded wait), then delivered
  /// through the router's duplicate check, which also drops a room's past
  /// from before it turned quiet. If the wait timed out, read again once
  /// the chunks are ready.
  Future<void> _readHiddenDmGap(
    int generation,
    int droppedAt,
    int readyTick,
  ) async {
    final router = ref.read(roomLiveRouterProvider);
    final hidden = [
      for (final roomId in ref.read(channelsProvider.notifier).hiddenDmIds)
        if (router.isQuietRoom(roomId)) roomId,
    ]..sort();
    if (hidden.isEmpty) return;
    final ready = await router.whenRoomsReadySince(hidden, readyTick);
    if (generation != _subscriptionGeneration) return;
    await _queryHiddenDmGap(router, hidden, droppedAt);
    if (ready) return;
    final lateReady = await router.whenRoomsReadySince(
      hidden,
      readyTick,
      limit: hiddenDmLateReadyLimit,
    );
    if (!lateReady || generation != _subscriptionGeneration) return;
    await _queryHiddenDmGap(router, hidden, droppedAt);
  }

  Future<void> _queryHiddenDmGap(
    RoomLiveRouter router,
    List<String> hidden,
    int droppedAt,
  ) async {
    try {
      final events = await ref.read(relaySessionProvider.notifier).queryRelay([
        for (
          var start = 0;
          start < hidden.length;
          start += kMaxExplicitChannelValues
        )
          NostrFilter(
            kinds: EventKind.channelMessageEventKinds,
            tags: {
              '#h': hidden.sublist(
                start,
                math.min(start + kMaxExplicitChannelValues, hidden.length),
              ),
            },
            since: droppedAt - meCatchUpLookbackSeconds,
            limit: 100,
          ),
      ]);
      router.routeQuietBackfill(events);
    } catch (error) {
      debugPrint('[ActivityNotifier] hidden-DM gap read failed: $error');
    }
  }

  /// Mentions, approvals and agent jobs addressed to me, live only on the
  /// account bundle: the feed fetch holds the history, and a catch-up that may
  /// have missed events (onGap) refreshes it. A failure leaves the feed
  /// working; the next connection subscribes again.
  Future<void> _subscribeAddressed(int generation) async {
    final myPk = ref.read(myPubkeyProvider);
    if (myPk == null) return;
    try {
      final unsubscribe = await ref
          .read(relaySessionProvider.notifier)
          .subscribe(
            NostrFilter(
              kinds: _addressedKinds,
              tags: {
                '#p': [myPk],
              },
              limit: 0,
            ),
            (event) => _handleAddressedLiveEvent(event, generation),
            onGap: () => _scheduleLiveRefresh(generation),
          );
      if (generation != _subscriptionGeneration) {
        unsubscribe();
        return;
      }
      _unsubscribeAddressed = unsubscribe;
    } catch (error) {
      if (generation == _subscriptionGeneration) {
        debugPrint('[ActivityNotifier] addressed subscription failed: $error');
      }
    }
  }

  /// Visible DMs ride the room bundle: a new message refreshes the feed (no
  /// REQ of their own). Registered before the fetch, so nothing between the
  /// read and the listener is lost. Returns the DMs the bundle did not cover
  /// yet: their messages before coverage can miss the fetch.
  List<String> _listenVisibleDms(int generation) {
    final dmIds = {
      for (final channel in _relayChannels(ref))
        if (channel.isPersonChat && channel.isMember) channel.id,
    };
    if (dmIds.isEmpty) return const [];
    _unsubscribeVisibleDms = ref.read(channelLiveEventsProvider).listen((
      event,
    ) {
      if (event.kind == 9 && dmIds.contains(event.channelId)) {
        _scheduleLiveRefresh(generation);
      }
    });
    final router = ref.read(roomLiveRouterProvider);
    return [
      for (final roomId in dmIds)
        if (!router.isRoomCovered(roomId)) roomId,
    ];
  }

  /// One refresh once the DMs not covered at fetch time are (at most
  /// [roomCoverageWaitLimit] later), for messages that fell in between.
  Future<void> _refreshOnDmCoverage(int generation, List<String> dms) async {
    await ref.read(roomLiveRouterProvider).whenRoomsCovered(dms);
    _scheduleLiveRefresh(generation);
  }

  void _handleAddressedLiveEvent(NostrEvent event, int generation) {
    _scheduleLiveRefresh(generation);
  }

  void _handleHiddenDmLiveEvent(NostrEvent event, int generation) {
    if (generation != _subscriptionGeneration) return;
    final myPk = ref.read(myPubkeyProvider);
    if (myPk == null || !isIncomingChannelMessageFromOther(event, myPk)) return;
    _scheduleLiveRefresh(generation);
    if (!ref.read(channelsProvider.notifier).hasLoaded) {
      unawaited(
        _resurfaceAfterChannelDiscovery(event, myPk, _dmResurfaceScope),
      );
      return;
    }
    unawaited(_resurfaceHiddenDm(event, myPk, generation));
  }

  Future<void> _resurfaceAfterChannelDiscovery(
    NostrEvent event,
    String myPk,
    String? expectedScope,
  ) async {
    try {
      await ref.read(channelsProvider.future);
      // The saved list drawn first is not the relay's.
      await waitForRelayChannelList(ref);
    } catch (error) {
      if (expectedScope == _dmResurfaceScope) {
        debugPrint(
          '[ActivityNotifier] channel discovery failed before DM resurface: $error',
        );
      }
      return;
    }
    if (expectedScope != _dmResurfaceScope) return;
    await _resurfaceHiddenDm(event, myPk, _subscriptionGeneration);
  }

  Future<void> _resurfaceHiddenDm(
    NostrEvent event,
    String myPk,
    int generation,
  ) async {
    final channelId = event.channelId;
    if (channelId == null || generation != _subscriptionGeneration) return;
    final channelsNotifier = ref.read(channelsProvider.notifier);
    if (!channelsNotifier.hasLoaded ||
        !channelsNotifier.hiddenDmIds.contains(channelId)) {
      return;
    }
    // Coalesce per channel within a generation: a concurrent follower for the
    // same DM marks the in-flight attempt for retry rather than being dropped,
    // so a failed reopen re-runs instead of leaving the row hidden. A follower
    // whose attempt was started by a superseded generation does not coalesce —
    // that attempt can never resume, so this generation starts a fresh one.
    final existing = _pendingDmResurfaceRetry[channelId];
    if (existing != null && existing.generation == generation) {
      existing.retry = true;
      return;
    }
    final pending = _PendingResurface(generation);
    _pendingDmResurfaceRetry[channelId] = pending;

    try {
      do {
        pending.retry = false;
        try {
          final members = await ref.read(
            channelMembersProvider(channelId).future,
          );
          if (generation != _subscriptionGeneration) return;
          final peers = dmPeerPubkeysFromMembers(
            members.map((member) => member.pubkey),
            myPk,
          );
          if (peers.isEmpty) return;
          final openedChannelId = await ref.read(dmResurfaceActionProvider)(
            peers.toList(),
          );
          if (generation != _subscriptionGeneration) return;
          if (openedChannelId != channelId) {
            throw StateError('Relay reopened a different DM conversation.');
          }
          return;
        } catch (error) {
          if (generation == _subscriptionGeneration) {
            debugPrint(
              '[ActivityNotifier] failed to resurface hidden DM $channelId: $error',
            );
          }
        }
      } while (pending.retry && generation == _subscriptionGeneration);
    } finally {
      // Only clear the entry if it is still the one this attempt installed; a
      // newer generation may have replaced it, and stomping that entry would
      // let its follower be dropped.
      if (identical(_pendingDmResurfaceRetry[channelId], pending)) {
        _pendingDmResurfaceRetry.remove(channelId);
      }
    }
  }

  void _scheduleLiveRefresh(int generation) {
    if (generation != _subscriptionGeneration) return;
    if (_buildingGeneration == generation) {
      _pendingDuringBuild = true;
      return;
    }
    _liveRefreshTimer?.cancel();
    _liveRefreshTimer = Timer(
      const Duration(milliseconds: 50),
      () => unawaited(_queueRefresh(generation)),
    );
  }

  Future<void> _queueRefresh(int generation) {
    if (generation != _subscriptionGeneration) return Future.value();
    _refreshQueued = true;
    final inFlight = _refreshInFlight;
    if (_refreshGeneration == generation && inFlight != null) {
      return inFlight;
    }

    _refreshGeneration = generation;
    final future = _drainRefreshQueue(generation);
    _refreshInFlight = future;
    return future;
  }

  Future<void> _drainRefreshQueue(int generation) async {
    try {
      do {
        _refreshQueued = false;
        try {
          final next = await _fetch();
          if (generation != _subscriptionGeneration) return;
          state = AsyncData(next);
        } catch (error) {
          if (generation != _subscriptionGeneration) return;
          debugPrint(
            '[ActivityNotifier] inbox refresh failed; retaining feed: $error',
          );
        }
      } while (_refreshQueued && generation == _subscriptionGeneration);
    } finally {
      if (_refreshGeneration == generation) {
        _refreshInFlight = null;
        _refreshGeneration = null;
        _refreshQueued = false;
      }
    }
  }

  void _clearLiveSubscriptions() {
    _liveRefreshTimer?.cancel();
    _liveRefreshTimer = null;
    _refreshInFlight = null;
    _refreshGeneration = null;
    _refreshQueued = false;
    _unsubscribeAddressed?.call();
    _unsubscribeAddressed = null;
    _unsubscribeVisibleDms?.call();
    _unsubscribeVisibleDms = null;
    _unsubscribeQuiet?.call();
    _unsubscribeQuiet = null;
  }

  /// Stable identity for the joined DM channel set: null while channels are
  /// loading, otherwise the sorted member-DM ids. Keeps unrelated channel
  /// updates from refetching the feed.
  static String? _dmChannelKey(AsyncValue<List<Channel>> channels) {
    final value = channels.asData?.value;
    if (value == null) return null;
    final ids = [
      for (final channel in value)
        if (channel.isPersonChat && channel.isMember) channel.id,
    ]..sort();
    return ids.join(',');
  }

  Future<HomeFeedResponse> _fetch() async {
    final myPk = ref.read(myPubkeyProvider);
    if (myPk == null) {
      return HomeFeedResponse(
        mentions: const [],
        needsAction: const [],
        activity: const [],
        agentActivity: const [],
      );
    }

    final session = ref.read(relaySessionProvider.notifier);

    // DM channels come from the channel list; while it is still loading the
    // DM source is skipped, and build() rebuilds when it resolves (see the
    // channelsProvider watch above).
    final dmChannelIds = [
      for (final channel in _relayChannels(ref))
        if (channel.isPersonChat && channel.isMember) channel.id,
    ];

    final filters = <NostrFilter>[
      // Mentions of me on user-visible channel content.
      NostrFilter(
        kinds: const [9, 40002, 1, 45001, 45003],
        tags: {
          '#p': [myPk],
        },
        limit: 50,
      ),
      // Workflow approvals addressed to me.
      NostrFilter(
        kinds: const [46010, 46011, 46012],
        tags: {
          '#p': [myPk],
        },
        limit: 20,
      ),
      // Agent job lifecycle events addressed to me.
      NostrFilter(
        kinds: const [43001, 43002, 43003, 43004, 43005, 43006],
        tags: {
          '#p': [myPk],
        },
        limit: 20,
      ),
      // Recent DM traffic (filtered to other senders below).
      if (dmChannelIds.isNotEmpty)
        NostrFilter(kinds: const [9], tags: {'#h': dmChannelIds}, limit: 30),
    ];

    // The HTTP bridge keeps each NIP-01 filter's independent limit while
    // executing the batch with bounded server-side concurrency. One request
    // here replaces the four simultaneous websocket history subscriptions that
    // otherwise compete with channel and preference startup sync.
    final events = await _queryWithWebSocketFallback(session, filters);

    bool isFromOther(NostrEvent e) =>
        e.pubkey.toLowerCase() != myPk.toLowerCase();
    bool isAddressedToMe(NostrEvent event) => event.tags.any(
      (tag) =>
          tag.length > 1 &&
          tag[0] == 'p' &&
          tag[1].toLowerCase() == myPk.toLowerCase(),
    );

    const mentionKinds = {9, 40002, 1, 45001, 45003};
    const needsActionKinds = {46010, 46011, 46012};
    const agentActivityKinds = {43001, 43002, 43003, 43004, 43005, 43006};
    final dmChannelIdSet = dmChannelIds.toSet();

    // Dedupe across sources by event id, keeping the higher-priority
    // category (needs_action > mention > agent_activity > activity).
    final byId = <String, FeedItem>{};
    void add(Iterable<NostrEvent> events, String category) {
      for (final event in events) {
        final existing = byId[event.id];
        if (existing != null &&
            categoryPriority(existing.category) <= categoryPriority(category)) {
          continue;
        }
        byId[event.id] = _feedItem(event, category: category);
      }
    }

    add(
      events.where(
        (event) =>
            needsActionKinds.contains(event.kind) && isAddressedToMe(event),
      ),
      'needs_action',
    );
    add(
      events.where(
        (event) =>
            mentionKinds.contains(event.kind) &&
            isAddressedToMe(event) &&
            isFromOther(event),
      ),
      'mention',
    );
    add(
      events.where(
        (event) =>
            agentActivityKinds.contains(event.kind) && isAddressedToMe(event),
      ),
      'agent_activity',
    );
    add(
      events.where(
        (event) =>
            event.kind == 9 &&
            dmChannelIdSet.contains(event.channelId) &&
            isFromOther(event),
      ),
      'activity',
    );

    final items = byId.values.toList()
      ..sort((a, b) => b.createdAt.compareTo(a.createdAt));

    return HomeFeedResponse(
      mentions: [
        for (final i in items)
          if (i.category == 'mention') i,
      ],
      needsAction: [
        for (final i in items)
          if (i.category == 'needs_action') i,
      ],
      activity: [
        for (final i in items)
          if (i.category == 'activity') i,
      ],
      agentActivity: [
        for (final i in items)
          if (i.category == 'agent_activity') i,
      ],
    );
  }

  Future<List<NostrEvent>> _queryWithWebSocketFallback(
    RelaySessionNotifier session,
    List<NostrFilter> filters,
  ) async {
    try {
      return await session.queryRelay(filters);
    } catch (error) {
      debugPrint(
        '[ActivityNotifier] batched history query failed; '
        'using bounded websocket fallback: $error',
      );
    }

    const fallbackConcurrency = 4;
    final events = <NostrEvent>[];
    for (var start = 0; start < filters.length; start += fallbackConcurrency) {
      final end = start + fallbackConcurrency < filters.length
          ? start + fallbackConcurrency
          : filters.length;
      final results = await Future.wait(
        filters.sublist(start, end).map((filter) async {
          try {
            return await session.fetchHistory(filter);
          } catch (_) {
            return const <NostrEvent>[];
          }
        }),
      );
      for (final result in results) {
        events.addAll(result);
      }
    }
    return events;
  }

  FeedItem _feedItem(NostrEvent event, {required String category}) {
    return FeedItem(
      id: event.id,
      kind: event.kind,
      pubkey: event.pubkey,
      content: event.content,
      createdAt: event.createdAt,
      channelId: event.channelId,
      channelName: '',
      tags: event.tags,
      category: category,
    );
  }

  Future<void> refresh() async {
    await _queueRefresh(_subscriptionGeneration);
  }
}

/// A generation-owned, in-flight hidden-DM resurface attempt. `generation` ties
/// the entry to the subscription that started it so a follower on a rebuilt
/// subscription never coalesces into a suspended attempt that will never
/// resume; `retry` records that a follower arrived mid-attempt.
class _PendingResurface {
  _PendingResurface(this.generation);

  final int generation;
  bool retry = false;
}

final activityProvider =
    AsyncNotifierProvider<ActivityNotifier, HomeFeedResponse>(
      ActivityNotifier.new,
    );

/// Conversation-grouped inbox rows derived from the raw feed. DM messages
/// group by DM channel so one conversation renders as one row.
final inboxItemsProvider = Provider<List<InboxItem>>((ref) {
  final feed = ref.watch(activityProvider).value;
  if (feed == null) return const [];
  final dmChannelIds = {
    for (final channel
        in ref.watch(channelsProvider).asData?.value ?? const <Channel>[])
      if (channel.isPersonChat) channel.id,
  };
  final blocked = ref.watch(blockedPubkeysProvider);
  return buildInboxItems(
    withoutBlockedAuthors(feed.all, blocked),
    isDmChannel: dmChannelIds.contains,
  );
});

/// [items] minus those written by someone in [blocked]: a blocked person's
/// messages never surface or count in Activity.
Iterable<FeedItem> withoutBlockedAuthors(
  Iterable<FeedItem> items,
  Set<String> blocked,
) => blocked.isEmpty
    ? items
    : items.where((item) => !blocked.contains(item.pubkey.toLowerCase()));

/// Recent main-timeline and thread traffic across joined channels. Loaded only
/// while the Channels + Threads view is open, so the default inbox and the
/// Home badge keep their addressed-to-me scope. Mirrors the desktop feed's
/// `activity` source.
final channelActivityProvider = FutureProvider.autoDispose<List<FeedItem>>((
  ref,
) async {
  ref.watch(relayConfigProvider);
  final connected = ref.watch(
    relaySessionProvider.select((s) => s.status == SessionStatus.connected),
  );
  final channelsNotifier = ref.read(channelsProvider.notifier);
  final channelKey = ref.watch(
    channelsProvider.select(
      (v) => channelsNotifier.showsSavedList ? null : _joinedChannelKey(v),
    ),
  );
  if (!connected || channelKey == null || channelKey.isEmpty) return const [];

  // One filter per channel so busy rooms cannot crowd quiet ones out of a
  // shared limit; batched to the relay's per-request filter cap.
  final filters = [
    for (final channelId in channelKey.split(','))
      NostrFilter(
        kinds: const [9, 40002, 45001, 45003],
        tags: {
          '#h': [channelId],
        },
        limit: _channelActivityPerChannelLimit,
      ),
  ];
  final session = ref.read(relaySessionProvider.notifier);
  Future<List<NostrEvent>> loadBatch(List<NostrFilter> batch) async {
    try {
      return await session.queryRelay(batch);
    } catch (error) {
      debugPrint(
        '[channelActivity] batched query failed; using websocket: $error',
      );
      final results = await Future.wait(batch.map(session.fetchHistory));
      return [for (final events in results) ...events];
    }
  }

  final batches = await Future.wait([
    for (var i = 0; i < filters.length; i += _relayFilterBatchSize)
      loadBatch(
        filters.sublist(i, math.min(i + _relayFilterBatchSize, filters.length)),
      ),
  ]);
  final events = [for (final batch in batches) ...batch];
  return [for (final event in events) _channelActivityItem(event)];
});

/// One room's recent traffic, refetched whenever its `lastMessageAt` moves.
/// [channelActivityProvider] loads once when the view opens; this keeps rooms
/// that got newer messages since then current, including the user's own
/// sends, which the addressed-to-me feed never carries. A rebuild keeps the
/// previous value while loading, so the row does not drop out meanwhile.
/// Mirrors desktop's per-room `conversation-room-activity` query.
final roomActivityRefreshProvider = FutureProvider.autoDispose
    .family<List<FeedItem>, String>((ref, channelId) async {
      ref.watch(
        channelsProvider.select(
          (channels) => channels.asData?.value
              .where((channel) => channel.id == channelId)
              .firstOrNull
              ?.lastMessageAt,
        ),
      );
      final filter = NostrFilter(
        kinds: const [9, 40002, 45001, 45003],
        tags: {
          '#h': [channelId],
        },
        limit: _channelActivityPerChannelLimit,
      );
      final session = ref.read(relaySessionProvider.notifier);
      List<NostrEvent> events;
      try {
        events = await session.queryRelay([filter]);
      } catch (error) {
        debugPrint(
          '[channelActivity] room query failed; using websocket: $error',
        );
        events = await session.fetchHistory(filter);
      }
      return [for (final event in events) _channelActivityItem(event)];
    });

FeedItem _channelActivityItem(NostrEvent event) => FeedItem(
  id: event.id,
  kind: event.kind,
  pubkey: event.pubkey,
  content: event.content,
  createdAt: event.createdAt,
  channelId: event.channelId,
  channelName: '',
  tags: event.tags,
  category: 'activity',
);

const _channelActivityPerChannelLimit = 30;
const _relayFilterBatchSize = 10;

/// Sorted ids of joined, non-archived channels (DMs included), or null while
/// channels are loading.
/// The relay's channel list, or none while only the saved list is drawn.
List<Channel> _relayChannels(Ref ref) =>
    ref.read(channelsProvider.notifier).showsSavedList
    ? const <Channel>[]
    : ref.read(channelsProvider).asData?.value ?? const <Channel>[];

String? _joinedChannelKey(AsyncValue<List<Channel>> channels) {
  final value = channels.asData?.value;
  if (value == null) return null;
  final ids = [
    for (final channel in value)
      if (channel.isMember && !channel.isArchived) channel.id,
  ]..sort();
  return ids.join(',');
}

/// Inbox rows for the Channels + Threads view: the regular feed plus each
/// room's and thread's latest message, with each channel's main timeline
/// collapsed to one row. One relay summary request feeds the rows; per-room
/// loading is the fallback for relays that do not serve the summary envelope.
final conversationInboxItemsProvider = Provider.autoDispose<List<InboxItem>>((
  ref,
) {
  final feed = ref.watch(activityProvider).value;
  final summary = ref.watch(inboxSummaryProvider);
  final channels =
      ref.watch(channelsProvider).asData?.value ?? const <Channel>[];
  final summaryRows = inboxSummaryFailed(summary)
      ? null
      : summary.value?.rows ?? const <InboxSummaryRow>[];
  final conversationActivity = summaryRows == null
      ? _perRoomConversationActivity(ref, channels)
      : inboxSummaryFeedItems(summaryRows);
  // Feed items win on duplicate ids: their category (mention, needs action)
  // is more specific than plain channel activity.
  final byId = <String, FeedItem>{
    for (final item in conversationActivity) item.id: item,
    for (final item in feed?.all ?? const <FeedItem>[]) item.id: item,
  };
  final dmChannelIds = {
    for (final channel in channels)
      if (channel.isPersonChat) channel.id,
  };
  final rows = collapseChannelMainRows(
    buildInboxItems(
      withoutBlockedAuthors(byId.values, ref.watch(blockedPubkeysProvider)),
      isDmChannel: dmChannelIds.contains,
    ),
  );
  return summaryRows == null
      ? rows
      : withInboxSummaryRoomRows(rows, summaryRows);
});

/// Per-room fallback: recent traffic of every joined room, plus a refetch of
/// each room that moved past the initial load (a new message, often the
/// user's own) so it re-sorts by its real latest activity.
List<FeedItem> _perRoomConversationActivity(Ref ref, List<Channel> channels) {
  final channelActivity =
      ref.watch(channelActivityProvider).value ?? const <FeedItem>[];
  final loadedLatestAt = <String, int>{};
  for (final item in channelActivity) {
    final channelId = item.channelId;
    if (channelId == null) continue;
    loadedLatestAt[channelId] = math.max(
      loadedLatestAt[channelId] ?? 0,
      item.createdAt,
    );
  }
  return [
    ...channelActivity,
    if (ref.watch(channelActivityProvider).hasValue)
      for (final channel in channels)
        if (channel.isMember &&
            !channel.isArchived &&
            (dateTimeToUnixSeconds(channel.lastMessageAt) ?? 0) >
                (loadedLatestAt[channel.id] ?? 0))
          ...?ref.watch(roomActivityRefreshProvider(channel.id)).value,
  ];
}
