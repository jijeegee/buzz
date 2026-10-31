import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/widgets.dart';
import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/community/community_provider.dart';
import '../../shared/message_store/message_store.dart'
    show MessageStoreLease, StoreCapture;
import '../../shared/message_store/message_store_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import '../../shared/push/push_presentation_cache.dart';
import '../../shared/push/push_presentation_export_recovery.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';
import '../../shared/utils/string_utils.dart';
import '../goals/goal_provider.dart';
import 'channel.dart';
import 'channel_live_events.dart';
import 'channel_management_provider.dart'
    show ChannelMember, channelDetailsProvider;
import 'channel_mutes/channel_mutes_provider.dart';
import 'channel_snapshot_codec.dart';
import 'deferred_store_work.dart';
import 'room_live_router.dart';
import 'huddle_channel_filter.dart';
import 'timeline_message.dart' show SystemEvent, SystemEventType;
import '../../shared/read_state/read_state_provider.dart';
import 'thread_follows/thread_follows_provider.dart';
import 'unread_badge/is_high_priority_event.dart';
import 'unread_badge/observed_unread_event.dart';
import 'unread_badge/should_notify_for_event.dart';

part 'channel_directory.dart';
part 'channel_list_recovery.dart';
part 'channel_list_requests.dart';
part 'channel_list_snapshot.dart';
part 'channel_push_cache.dart';
part 'channel_member_snapshots.dart';
part 'channels_provider_lifecycle.dart';

const _channelTypeOrder = {'stream': 0, 'forum': 1, 'dm': 2};
const _unreadCatchUpLimit = 1000;

/// How long an unread catch-up waits for the read markers to load before
/// giving up; the next refresh (reconnect backstop) starts a new one.
const _readStateReadyTimeout = Duration(seconds: 30);
const _participatedRootIdsPrefix = 'kuzz-thread-participation.v1';
const _authoredRootIdsPrefix = 'kuzz-thread-authored.v1';

/// Loads the user's channel list from the relay over WebSocket.
///
/// Membership loading resolves kind:39002 events tagged `#p:<my-pubkey>`,
/// then fetches kind:39000 metadata for those channel ids.
///
/// The paginated kind:39000 directory is fetched separately when Browse
/// channels opens, so discovery never delays the main Conversations screen.
/// Live updates are layered on top via chunked subscriptions on the `#h` tag
/// for any visible channel event kind. Chunks stay within the relay's explicit
/// channel cap and incoming events bump `lastMessageAt` for their channel.
class ChannelsNotifier extends AsyncNotifier<List<Channel>> {
  final _pushExport = PushPresentationExportRecovery();
  bool _pushCacheExporting = false;
  bool _pushCacheDirty = false;

  static const _backstopInterval = Duration(seconds: 60);

  /// Wait between a live metadata-change system message and the refetch it
  /// triggers. The relay posts the system message before it republishes the
  /// group's kind:39000, so an immediate refetch could read the old metadata.
  @visibleForTesting
  static Duration liveMetadataRefreshDelay = const Duration(seconds: 1);
  Timer? _metadataRefreshTimer;

  final Map<String, _LiveChunkSubscription> _liveSubscriptionsByChunk = {};
  Future<void> _liveSubscriptionQueue = Future.value();
  Set<String> _desiredLiveChannelIds = const {};
  int _subscriptionVersion = 0;
  int _nextLiveChunkGeneration = 0;
  final Set<String> _terminallyClosedLiveChunks = {};
  String? _subscriptionRelayBaseUrl;
  Timer? _backstopTimer;
  final Map<String, int> _latestObservedByChannel = {};
  final Map<String, Map<String, ObservedUnreadEvent>>
  _observedUnreadEventsByChannel = {};
  Set<String> _participatedRootIds = {};
  Set<String> _authoredRootIds = {};
  String? _threadInterestPubkey;
  bool _hasLoaded = false;
  late final _listSnapshot = _ChannelListSnapshot(this);
  late final _recovery = _ChannelListRecovery(this);
  String? _memberSnapshotRelayBaseUrl;
  String? _memberSnapshotPubkey;
  Map<String, List<ChannelMember>> _memberSnapshotsByChannelId = const {};
  List<NostrEvent> _directoryMetas = const [];
  Set<String> _hiddenDmIds = const {};

  /// Fences directory responses to the relay and identity that requested them.
  late final _ChannelRefreshCoordinator _refreshCoordinator =
      _ChannelRefreshCoordinator.forRef(ref);

  // Expose the notifier's protected Ref only to same-library extensions used to
  // keep lifecycle code below the file-size ratchet.
  Ref get _lifecycleRef => ref;

  /// The room router of this build's relay and identity (it rebuilds on the
  /// same two), held so coverage can be cleared from `onDispose`, where
  /// reading providers is not allowed.
  RoomLiveRouter? _coverageRouter;

  /// The member snapshot already returned while loading the channel list.
  ///
  /// Mention autocomplete can use this synchronously while its independent
  /// channel-member refresh is still in flight.
  List<ChannelMember> cachedMembersForChannel(String channelId) =>
      _memberSnapshotsByChannelId[channelId] ?? const [];

  Map<String, int> get latestObservedByChannel =>
      Map.unmodifiable(_latestObservedByChannel);

  Set<String> get hiddenDmIds => Set.unmodifiable(_hiddenDmIds);

  /// Whether the list is the relay's, not the saved one drawn before it.
  bool get hasLoaded => _hasLoaded;

  /// Whether the list shown is the saved one (or, right after an account
  /// switch, none yet), drawn before the relay's list replaces it.
  bool get showsSavedList => _listSnapshot.shown;

  /// Rooms the relay's list shows joined; none while the saved list is
  /// shown, so it never keeps a room from the left-room purge.
  Set<String> get relayJoinedChannelIds => _listSnapshot.shown
      ? const {}
      : {
          for (final channel in state.value ?? const <Channel>[])
            if (channel.isMember) channel.id,
        };

  void _showList(List<Channel> channels) => state = AsyncData(channels);

  Map<String, Map<String, ObservedUnreadEvent>>
  get observedUnreadEventsByChannel =>
      Map<String, Map<String, ObservedUnreadEvent>>.unmodifiable({
        for (final entry in _observedUnreadEventsByChannel.entries)
          entry.key: Map<String, ObservedUnreadEvent>.unmodifiable(entry.value),
      });

  @override
  Future<List<Channel>> build() async {
    final relayBaseUrl = ref.watch(relayConfigProvider).baseUrl;
    final pubkey = ref.watch(myPubkeyProvider)?.toLowerCase();
    _coverageRouter = ref.read(roomLiveRouterProvider);
    if (_memberSnapshotRelayBaseUrl != relayBaseUrl ||
        _memberSnapshotPubkey != pubkey) {
      final switched = _memberSnapshotPubkey != null;
      _memberSnapshotRelayBaseUrl = relayBaseUrl;
      _memberSnapshotPubkey = pubkey;
      // Another account or server: its list is not the one shown, so the
      // previous one goes at once (the saved list of this one follows).
      _hasLoaded = false;
      _listSnapshot.resetScope();
      _recovery.resetScope();
      if (switched) {
        _listSnapshot.shown = true;
        _showList(const []);
      }
      _memberSnapshotsByChannelId = const {};
      _directoryMetas = const [];
      _hiddenDmIds = const {};
      // Retire any in-flight directory request: its response describes the
      // previous relay or identity and must not reach this scope's state.
      _refreshCoordinator.retireInFlight();
    }
    final connected = Completer<void>();
    final sessionState = ref.read(relaySessionProvider);
    final waitingForInitialConnection =
        sessionState.status != SessionStatus.connected;
    ref.listen(relaySessionProvider, (previous, next) {
      if (next.status != SessionStatus.connected) return;
      if (waitingForInitialConnection &&
          !_hasLoaded &&
          !connected.isCompleted) {
        connected.complete();
      } else if (previous?.status != SessionStatus.connected) {
        // A new authenticated connection can change relay admission policy.
        // Ordinary refreshes must not retry an unchanged terminal rejection.
        _terminallyClosedLiveChunks.clear();
        unawaited(_backstopRefresh());
      }
    });

    // Re-fetch when the app returns to foreground so channels created on
    // another device while mobile was backgrounded appear immediately.
    ref.listen(appLifecycleProvider, (prev, next) {
      if (next == AppLifecycleState.resumed) {
        refresh();
      }
    });

    listenSelf((_, next) => _listSnapshot.listShown(next));
    ref.onDispose(() {
      _listSnapshot.dispose();
      _recovery.dispose();
      _clearLiveSubscriptions();
      _latestObservedByChannel.clear();
      _observedUnreadEventsByChannel.clear();
      _backstopTimer?.cancel();
      _backstopTimer = null;
      _metadataRefreshTimer?.cancel();
      _metadataRefreshTimer = null;
    });

    // The saved list draws while the socket and the relay's list are on
    // their way.
    if (!_hasLoaded) _listSnapshot.paint(relayBaseUrl, pubkey);
    if (sessionState.status != SessionStatus.connected) {
      // Keep this scope's list visible through a reconnect.
      if (_hasLoaded) return state.value ?? const [];
      await connected.future;
    }

    try {
      return await _fetch(subscribeLive: true);
    } on _StaleChannelRefresh {
      rethrow;
    } catch (error) {
      // A failed relay list keeps the saved one shown and asks again.
      final scope = (_memberSnapshotRelayBaseUrl, _memberSnapshotPubkey);
      await _listSnapshot.pending;
      if (scope != (_memberSnapshotRelayBaseUrl, _memberSnapshotPubkey)) {
        throw const _StaleChannelRefresh();
      }
      if (!_listSnapshot.shown) rethrow;
      _recovery.failed(error);
      return state.value ?? const [];
    }
  }

  /// Asks for the relay's channel list now (the pages' retry).
  Future<void> retryRelayList() => _recovery.retryNow();

  Future<List<Channel>> _fetch({
    bool subscribeLive = false,
    bool fetchLastMessage = true,
    bool fetchDirectory = false,
  }) async {
    final channels = await _fetchChannels(
      subscribeLive: subscribeLive,
      fetchLastMessage: fetchLastMessage,
      fetchDirectory: fetchDirectory,
    );
    _hasLoaded = true;
    _listSnapshot.shown = false;
    _recovery.loaded();
    return channels;
  }

  Future<List<Channel>> _fetchChannels({
    bool subscribeLive = false,
    bool fetchLastMessage = true,
    bool fetchDirectory = false,
  }) async {
    final myPk = ref.read(myPubkeyProvider);
    if (myPk == null) throw StateError('No signing identity available');
    final communityID = ref.read(activeCommunityProvider).value?.id;
    _loadThreadInterestStores(myPk);

    final session = ref.read(relaySessionProvider.notifier);
    final storeScope = session.storeScope;

    // Acquire request ownership before the first relay await. Every channel-list
    // path uses this fence so completion order cannot let an older ordinary,
    // directory, or reconnect refresh replace a newer membership list.
    final fence = await _refreshCoordinator.beginRefresh(
      fetchesDirectory: fetchDirectory,
    );

    // Step 1: find the channels I'm a member of via kind:39002.
    final memberships = await _fenced(
      fence,
      _fetchChannelMemberships(session, myPk),
    );
    final memberChannelIds = memberships
        .map((e) => e.getTagValue('d'))
        .whereType<String>()
        .toSet();
    _cacheMemberSnapshots(memberships, replaceAll: true);
    _forgetStoredRoomsMissingFrom(
      ref,
      storeScope,
      myPk,
      memberships,
      memberChannelIds.union(relayJoinedChannelIds),
    );

    // Step 2: the requests that need only the membership list, at once.
    final parts = await _fetchMembershipParts(
      session,
      fence,
      memberChannelIds.toList(),
      myPk,
      fetchDirectory: fetchDirectory,
    );
    final memberMetas = parts.memberMetas;
    if (parts.directory case final metas?) _directoryMetas = metas;
    final hiddenDmIds = parts.hiddenDmIds;
    final memberEvents = parts.memberEvents;
    final huddleStarts = parts.huddleStarts;
    _hiddenDmIds = Set.unmodifiable(hiddenDmIds);

    // Merge and dedupe by `d` tag. Kind:39000 is parameterized-replaceable,
    // but stale revisions from before the relay's d_tag backfill can linger.
    final latestMetaPerId = <String, NostrEvent>{};
    for (final event in [...memberMetas, ..._directoryMetas]) {
      if (event.kind != 39000) continue;
      final id = event.getTagValue('d');
      if (id == null) continue;
      final existing = latestMetaPerId[id];
      if (existing == null ||
          event.createdAt > existing.createdAt ||
          (event.createdAt == existing.createdAt &&
              event.id.compareTo(existing.id) < 0)) {
        latestMetaPerId[id] = event;
      }
    }
    final dedupedMetas = latestMetaPerId.values.toList();
    final huddleBackingIds = huddleBackingChannelIds(
      huddleStarts,
      memberEvents,
    );

    List<Channel> listedChannels(Map<String, String> displayNames) => [
      for (final event in dedupedMetas)
        if (event.getTagValue('d') case final id?)
          if (_channelFromMeta(
                event,
                isMember: memberChannelIds.contains(id),
                displayNames: displayNames,
              )
              case final channel
              when (channel.isMember ||
                      (!channel.isPrivate && !channel.isDm)) &&
                  !(channel.isDm && hiddenDmIds.contains(channel.id)) &&
                  !(huddleBackingIds.contains(channel.id) &&
                      channel.isStream &&
                      channel.isPrivate))
            // Ephemeral (TTL) channels stay listed with their badge, as on
            // desktop.
            channel,
    ];

    // Step 3: DM participant names (from saved profiles; only unknown people
    // are asked for) and, unless this is a backstop refresh, the most recent
    // message per channel, at once. kind:39000 metadata carries no message
    // timestamps; without them unread detection and badges see every channel
    // as empty. Backstop refreshes skip them: live subscriptions keep
    // lastMessageAt current after the initial load.
    final activeChannels = [
      for (final channel in listedChannels(const {}))
        if (channel.isMember && !channel.isArchived) channel,
    ];
    final named = await _fenced(
      fence,
      Future.wait<Object?>([
        _resolveDmDisplayNames(
          session,
          fence,
          dedupedMetas,
          myPk,
          ref.read(userCacheProvider),
          ref.read(userCacheProvider.notifier).cacheProfileEvent,
        ),
        fetchLastMessage
            ? _fetchLastMessageEvents(
                session,
                activeChannels,
                isCurrent: () => fence.isCurrent,
              )
            : Future.value(),
      ]),
    );
    final dmNames =
        named[0] as ({Map<String, String> names, Set<String> fromKnown});
    final channels = listedChannels(dmNames.names);

    // Use the membership snapshots already fetched above for both Huddle
    // linkage validation and member-count hydration.
    if (memberEvents.isNotEmpty) _cacheMemberSnapshots(memberEvents);
    unawaited(
      _exportPushCache(communityID, dedupedMetas, [
        ...memberships,
        ...memberEvents,
      ]),
    );
    final memberCounts = _memberCountsByChannelId(memberEvents);
    for (var i = 0; i < channels.length; i++) {
      final count = memberCounts[channels[i].id];
      if (count != null) {
        channels[i] = channels[i].copyWith(memberCount: count);
      }
      // A group chat's 39000 lists no people; its members (39002 order)
      // draw its title and photo, as a DM's participants do.
      if (channels[i].isGroupChat) {
        channels[i] = channels[i].copyWith(
          participantPubkeys: [
            for (final member in cachedMembersForChannel(channels[i].id))
              member.pubkey.toLowerCase(),
          ],
        );
      }
    }

    if (fetchLastMessage) {
      final channelById = {
        for (final channel in activeChannels) channel.id: channel,
      };
      final events = named[1] as List<NostrEvent>?;
      final lastMessageMap = <String, int>{};
      // An unavailable batch keeps the timestamps already known rather than
      // installing channels whose latest message looks absent.
      if (events == null) {
        for (final channel in state.value ?? const <Channel>[]) {
          final at = channel.lastMessageAt;
          if (at != null) {
            lastMessageMap[channel.id] = at.millisecondsSinceEpoch ~/ 1000;
          }
        }
      }
      final mutedChannelIds = _mutedChannelIds();
      for (final event in events ?? const <NostrEvent>[]) {
        final channelId = event.channelId;
        if (channelId == null) continue;
        final channel = channelById[channelId];
        if (channel == null) continue;
        if (channel.isForum &&
            !shouldNotifyForEvent(
              event,
              myPk,
              mutedChannelIds: mutedChannelIds,
              channelId: channelId,
            )) {
          continue;
        }
        final current = lastMessageMap[channelId];
        if (current == null || event.createdAt > current) {
          lastMessageMap[channelId] = event.createdAt;
        }
      }

      for (var i = 0; i < channels.length; i++) {
        final ts = lastMessageMap[channels[i].id];
        if (ts != null) {
          channels[i] = channels[i].copyWith(
            lastMessageAt: DateTime.fromMillisecondsSinceEpoch(
              ts * 1000,
              isUtc: true,
            ),
          );
        }
      }
    }

    channels.sort((left, right) {
      final typeOrder =
          (_channelTypeOrder[left.channelType] ?? 99) -
          (_channelTypeOrder[right.channelType] ?? 99);
      if (typeOrder != 0) return typeOrder;
      // Case-insensitive to match desktop's `localeCompare` ordering.
      return left.name.toLowerCase().compareTo(right.name.toLowerCase());
    });

    // Invalidate `channelDetailsProvider` entries whose archived or goals
    // state flipped since the last fetch. Required because `channelDetailsProvider` is a
    // separate Riverpod cache and `Channel.mergeDetails(details)` overwrites
    // archivedAt from the cached details — so an active-then-archived channel
    // (e.g. TTL auto-archive by the relay reaper) could keep showing compose
    // and manage actions in the detail view until the cache expired naturally.
    //
    // The same holds for the group photo, name, description and topic: a
    // change made on another device must reach the header and details, which
    // read the merged details, not only the Chats rows.
    // Re-check before the first write that other providers can observe. Every
    // await above is fenced, but the switch can also land in the synchronous
    // gap, so the guard sits immediately before the write rather than only
    // after the await.
    fence.ensureCurrent();

    final prevById = <String, Channel>{
      for (final c in state.value ?? const <Channel>[]) c.id: c,
    };
    for (final channel in channels) {
      final prev = prevById[channel.id];
      if (prev != null &&
          (prev.isArchived != channel.isArchived ||
              prev.goalsEnabled != channel.goalsEnabled ||
              prev.historyAll != channel.historyAll ||
              prev.picture != channel.picture ||
              prev.name != channel.name ||
              prev.description != channel.description ||
              prev.topic != channel.topic)) {
        ref.invalidate(channelDetailsProvider(channel.id));
      }
      // A tree read while goals were off is empty (the relay hides it), so
      // drop it when the room's goals flip.
      if (prev != null && prev.goalsEnabled != channel.goalsEnabled) {
        ref.invalidate(goalTreeProvider(channel.id));
      }
    }

    // Layer live delivery onto the already-built snapshot without making EOSE
    // part of the provider's readiness path. Every refresh queues a generation;
    // a later refresh or scope switch retires older queued/in-flight work.
    if (subscribeLive) {
      unawaited(_subscribeLive(channels, fence));
    }
    _recovery.recheckDmNames(dmNames.fromKnown);
    // Guard the provider-state write in `retryDirectory` and `build`: the
    // caller assigns whatever this returns, so the last check belongs here.
    fence.ensureCurrent();
    return channels;
  }

  /// Fetches each channel's independent latest-message window in one HTTP
  /// bridge request. The relay preserves NIP-01 per-filter limits while
  /// executing the filters with bounded concurrency, avoiding an unbounded
  /// burst of websocket REQs on communities with many channels.
  Future<List<NostrEvent>?> _fetchLastMessageEvents(
    RelaySessionNotifier session,
    List<Channel> channels, {
    required bool Function() isCurrent,
  }) async {
    if (channels.isEmpty) return const [];

    final filters = [
      for (final channel in channels)
        NostrFilter(
          kinds: EventKind.channelMessageEventKinds,
          tags: {
            '#h': [channel.id],
          },
          limit: channel.isDm ? 1 : 20,
        ),
    ];

    return _fetchChannelHistoryBatch(
      session,
      filters,
      operation: 'latest-message query',
      isCurrent: isCurrent,
    );
  }

  /// Returns null when the batch is unavailable: it hit a relay deadline, or
  /// it stayed rate-limited after its one retry.
  ///
  /// [isCurrent] is checked before the rate-limit retry, so a refresh retired
  /// during the wait sends nothing more.
  Future<List<NostrEvent>?> _fetchChannelHistoryBatch(
    RelaySessionNotifier session,
    List<NostrFilter> filters, {
    required String operation,
    required bool Function() isCurrent,
  }) async {
    if (filters.isEmpty) return const [];

    try {
      return await session.queryRelay(filters);
    } catch (error) {
      // Per-filter fallback would re-run the timed-out work another way.
      if (isRelayDeadlineError(error)) {
        debugPrint('[ChannelsNotifier] batched $operation hit the deadline');
        return null;
      }
      // Back-pressure: one request per room would only add to it. The same
      // single batch goes once more after the relay's hint; failing again,
      // the batch is unavailable and what is shown stays.
      if (isRelayRateLimitError(error)) {
        final delay = relayRateLimitedBatchRetryDelay(error);
        if (delay == null) {
          debugPrint('[ChannelsNotifier] batched $operation rate-limited');
          return null;
        }
        await Future<void>.delayed(delay);
        if (!isCurrent()) return null;
        try {
          return await session.queryRelay(filters);
        } catch (retryError) {
          debugPrint(
            '[ChannelsNotifier] batched $operation retry failed: $retryError',
          );
          return null;
        }
      }
      debugPrint(
        '[ChannelsNotifier] batched $operation failed; '
        'using bounded websocket fallback: $error',
      );
    }

    const fallbackConcurrency = 4;
    final events = <NostrEvent>[];
    var deadline = false;
    for (var start = 0; start < filters.length; start += fallbackConcurrency) {
      // Stop sending once a filter hit the deadline; partial events are not
      // a result.
      if (deadline) return null;
      final end = min(start + fallbackConcurrency, filters.length);
      final results = await Future.wait(
        filters.sublist(start, end).map((filter) async {
          try {
            return await session.fetchHistory(filter);
          } catch (error) {
            // A timed-out filter makes the batch unavailable, not empty.
            if (isRelayDeadlineError(error)) deadline = true;
            return const <NostrEvent>[];
          }
        }),
      );
      for (final result in results) {
        events.addAll(result);
      }
    }
    if (deadline) return null;
    return events;
  }

  /// Build a [Channel] from a kind:39000 metadata event.
  ///
  /// [displayNames] maps lowercase participant pubkey → resolved label and is
  /// used to populate [Channel.participants] for DMs so [Channel.displayLabel]
  /// can render real names instead of the relay-canonical "DM" name.
  Channel _channelFromMeta(
    NostrEvent event, {
    required bool isMember,
    Map<String, String> displayNames = const {},
  }) {
    final data = ChannelData.fromEvent(event);
    final participants = data.channelType == 'dm'
        ? [
            for (final pk in data.participantPubkeys)
              displayNames[pk.toLowerCase()] ?? shortPubkey(pk),
          ]
        : const <String>[];
    return Channel(
      id: data.id,
      name: data.name,
      channelType: data.channelType,
      visibility: data.visibility,
      description: data.description,
      topic: data.topic,
      createdBy: event.pubkey,
      createdAt: DateTime.fromMillisecondsSinceEpoch(
        event.createdAt * 1000,
        isUtc: true,
      ),
      memberCount: 0,
      lastMessageAt: null,
      // `archivedAt` doubles as both the archived-state flag and the timestamp.
      // The kind:39000 metadata only carries `["archived", "true"]`, not the
      // moment of archival, so we stamp the event's `createdAt` — that's when
      // the relay republished the metadata, which is the closest signal we have.
      archivedAt: data.isArchived
          ? DateTime.fromMillisecondsSinceEpoch(
              event.createdAt * 1000,
              isUtc: true,
            )
          : null,
      participants: participants,
      participantPubkeys: data.participantPubkeys,
      isMember: isMember,
      ttlSeconds: data.ttlSeconds,
      ttlDeadline: data.ttlDeadline,
      goalsEnabled: data.goalsEnabled,
      picture: data.picture,
      groupChat: data.groupChat,
      historyAll: data.historyAll,
    );
  }

  /// Backfills unread badges for the channels this refresh just installed.
  ///
  /// Runs detached from the refresh that starts it, so the lifecycle token
  /// captured below is what keeps a response that outlived its refresh from
  /// writing unread state into whatever the user is looking at now. Every
  /// refresh path starts a catch-up, including the initial load, the ordinary
  /// membership refresh a join performs and the reconnect backstop, so the
  /// token is unconditional rather than tied to discovery. A retired refresh
  /// returns instead of throwing: nothing awaits this future, so a thrown
  /// [_StaleChannelRefresh] would only surface as an unhandled error.
  ///
  /// The request fence and subscription generation are passed from the refresh
  /// that installed [channels], preserving request ownership after detachment.
  Future<void> _catchUpUnreadEvents(
    List<Channel> channels,
    _ChannelRefreshFence fence,
    int subscriptionGeneration,
  ) async {
    if (!ref.mounted) return;
    final myPk = ref.read(myPubkeyProvider);
    if (myPk == null) return;

    final session = ref.read(relaySessionProvider.notifier);
    final mutedChannelIds = _mutedChannelIds();
    final ReadStateState? readState;
    try {
      readState = await _loadedReadState();
    } catch (error) {
      debugPrint('[ChannelsNotifier] unread catch-up skipped: $error');
      return;
    }
    // Without loaded markers every message would be caught up as unread.
    if (readState == null ||
        !ref.mounted ||
        _isCatchUpRetired(fence, subscriptionGeneration)) {
      return;
    }
    final activeChannels = [
      for (final channel in channels)
        if (channel.isMember && !channel.isArchived) channel,
    ];
    final channelById = {
      for (final channel in activeChannels) channel.id: channel,
    };
    final readAtByChannel = {
      for (final channel in activeChannels)
        channel.id: readState.effectiveTimestamp(channel.id),
    };
    final filters = [
      for (final channel in activeChannels)
        NostrFilter(
          kinds: EventKind.channelMessageEventKinds,
          tags: {
            '#h': [channel.id],
          },
          since: (readAtByChannel[channel.id] ?? -1) + 1,
          limit: _unreadCatchUpLimit,
        ),
    ];

    try {
      final events = await _fetchChannelHistoryBatch(
        session,
        filters,
        operation: 'unread catch-up',
        isCurrent: () =>
            ref.mounted && !_isCatchUpRetired(fence, subscriptionGeneration),
      );
      // Unavailable: keep the unread state already observed.
      if (events == null) return;
      // The relay round-trip above is the window Jed's probes park in: a newer
      // refresh, a community switch or an identity switch here means every
      // write below belongs to a channel list the user has left.
      if (!ref.mounted || _isCatchUpRetired(fence, subscriptionGeneration)) {
        return;
      }

      for (final event in events) {
        if (event.pubkey.toLowerCase() == myPk.toLowerCase()) {
          _recordSelfThreadInterest(event, myPk);
        }
      }

      final recordedAtByChannel = <String, int>{};
      for (final event in events) {
        final channelId = event.channelId;
        if (channelId == null) continue;
        final channel = channelById[channelId];
        if (channel == null) continue;
        final readAt = readAtByChannel[channelId];
        if (event.pubkey.toLowerCase() == myPk.toLowerCase()) continue;
        if (readAt != null && event.createdAt <= readAt) continue;
        if (!shouldNotifyForEvent(
          event,
          myPk,
          participatedRootIds: _participatedRootIds,
          followedRootIds: _followedRootIds(),
          authoredRootIds: _authoredRootIds,
          mutedChannelIds: mutedChannelIds,
          channelId: channel.id,
        )) {
          continue;
        }
        _recordUnreadEvent(channel, event, myPk);
        recordedAtByChannel[channelId] = max(
          recordedAtByChannel[channelId] ?? 0,
          event.createdAt,
        );
      }
      // Republish only when this catch-up actually changed unread state. A
      // batch that recorded nothing has nothing to show, and a failed or
      // superseded batch must not repaint another refresh's list: the retired
      // check above already returned in that case, and no await separates it
      // from here, so a second check would be dead code.
      // Recorded events also advance `lastMessageAt`, as live events do: a
      // deadline-unavailable latest-message batch leaves it unset.
      if (recordedAtByChannel.isNotEmpty) {
        state = state.whenData(
          (channels) => [
            for (final channel in channels)
              _advanceLastMessageAt(channel, recordedAtByChannel[channel.id]),
          ],
        );
      }
    } catch (error) {
      if (!ref.mounted) return;
      debugPrint('[ChannelsNotifier] unread catch-up failed: $error');
    }
  }

  /// The read state once its markers have loaded, or `null` when they do
  /// not load within [_readStateReadyTimeout].
  Future<ReadStateState?> _loadedReadState() async {
    final current = ref.read(readStateProvider);
    if (current.isReady) return current;
    final loaded = Completer<ReadStateState>();
    final subscription = ref.listen(readStateProvider, (_, next) {
      if (next.isReady && !loaded.isCompleted) loaded.complete(next);
    });
    try {
      return await loaded.future.timeout(_readStateReadyTimeout);
    } on TimeoutException {
      debugPrint('[ChannelsNotifier] read state not loaded; catch-up skipped');
      return null;
    } finally {
      subscription.close();
    }
  }

  static Channel _advanceLastMessageAt(Channel channel, int? createdAt) {
    if (createdAt == null) return channel;
    final at = DateTime.fromMillisecondsSinceEpoch(
      createdAt * 1000,
      isUtc: true,
    );
    final current = channel.lastMessageAt;
    if (current != null && !at.isAfter(current)) return channel;
    return channel.copyWith(lastMessageAt: at);
  }

  void _handleLiveEvent(NostrEvent event) {
    final channelId = event.channelId;
    if (channelId == null) return;

    if (_changesChannelMetadata(event) ||
        _changesGroupChatMembers(event, channelId)) {
      _scheduleMetadataRefresh();
    }

    final myPk = ref.read(myPubkeyProvider);
    final mutedChannelIds = _mutedChannelIds();

    state = state.whenData((channels) {
      final idx = channels.indexWhere((c) => c.id == channelId);
      if (idx == -1) {
        refresh();
        return channels;
      }
      final updated = List<Channel>.of(channels);
      final channel = updated[idx];

      if (myPk != null && event.pubkey.toLowerCase() == myPk.toLowerCase()) {
        _recordSelfThreadInterest(event, myPk);
      }

      // Chats recency describes messages independently of notifications:
      // own sends, muted chats and unfollowed replies count too.
      if (!channel.isForum &&
          EventKind.channelMessageEventKinds.contains(event.kind)) {
        updated[idx] = _advanceLastMessageAt(channel, event.createdAt);
      }

      if (myPk != null &&
          shouldNotifyForEvent(
            event,
            myPk,
            participatedRootIds: _participatedRootIds,
            followedRootIds: _followedRootIds(),
            authoredRootIds: _authoredRootIds,
            mutedChannelIds: mutedChannelIds,
            channelId: channel.id,
          )) {
        _recordUnreadEvent(channel, event, myPk);
        if (channel.isForum) {
          updated[idx] = _advanceLastMessageAt(channel, event.createdAt);
        }
      }

      return updated;
    });
  }

  /// Whether [event] is a system message announcing a change to the group's
  /// kind:39000 metadata (photo, topic, purpose or earlier-messages setting).
  static bool _changesChannelMetadata(NostrEvent event) {
    if (event.kind != EventKind.systemMessage) return false;
    return switch (SystemEvent.fromContent(event.content)?.type) {
      SystemEventType.pictureChanged ||
      SystemEventType.historyChanged ||
      SystemEventType.topicChanged ||
      SystemEventType.purposeChanged => true,
      _ => false,
    };
  }

  /// Whether [event] announces a member change in a group chat, whose
  /// members draw its title and photo.
  bool _changesGroupChatMembers(NostrEvent event, String channelId) {
    if (event.kind != EventKind.systemMessage) return false;
    final isGroupChat =
        state.value?.any((c) => c.id == channelId && c.isGroupChat) ?? false;
    if (!isGroupChat) return false;
    return switch (SystemEvent.fromContent(event.content)?.type) {
      SystemEventType.memberJoined ||
      SystemEventType.memberLeft ||
      SystemEventType.memberRemoved => true,
      _ => false,
    };
  }

  /// Refetches the channel list (and with it the changed kind:39000) shortly
  /// after a metadata-change system message, so another device's change shows
  /// without a restart. The message itself is only a trigger: anyone may post
  /// a kind:40099, so the new values come from the relay-signed metadata.
  /// Changes that arrive while a refetch is pending share it.
  void _scheduleMetadataRefresh() {
    if (_metadataRefreshTimer?.isActive ?? false) return;
    _metadataRefreshTimer = Timer(liveMetadataRefreshDelay, () {
      _metadataRefreshTimer = null;
      if (ref.mounted) unawaited(_backstopRefresh());
    });
  }

  Set<String> _mutedChannelIds() => {
    for (final entry in ref.read(channelMutesProvider).store.channels.entries)
      if (entry.value.muted) entry.key,
  };

  Set<String> _followedRootIds() =>
      ref.read(threadFollowsProvider).followedRootIds;

  void _loadThreadInterestStores(String pubkey) {
    final normalizedPubkey = pubkey.toLowerCase();
    if (_threadInterestPubkey == normalizedPubkey) return;
    _threadInterestPubkey = normalizedPubkey;
    try {
      final prefs = ref.read(savedPrefsProvider);
      _participatedRootIds = _readRootIdSet(
        prefs.getString('$_participatedRootIdsPrefix:$normalizedPubkey'),
      );
      _authoredRootIds = _readRootIdSet(
        prefs.getString('$_authoredRootIdsPrefix:$normalizedPubkey'),
      );
    } catch (_) {
      _participatedRootIds = {};
      _authoredRootIds = {};
    }
  }

  void _recordSelfThreadInterest(NostrEvent event, String pubkey) {
    final ref = event.threadReference;
    final target = ref.rootId != null ? _participatedRootIds : _authoredRootIds;
    final id = ref.rootId ?? event.id;
    if (!target.add(id)) return;
    _writeThreadInterestStores(pubkey);
  }

  void _writeThreadInterestStores(String pubkey) {
    final normalizedPubkey = pubkey.toLowerCase();
    try {
      final prefs = ref.read(savedPrefsProvider);
      prefs.setString(
        '$_participatedRootIdsPrefix:$normalizedPubkey',
        _encodeRootIdSet(_participatedRootIds),
      );
      prefs.setString(
        '$_authoredRootIdsPrefix:$normalizedPubkey',
        _encodeRootIdSet(_authoredRootIds),
      );
    } catch (_) {
      // Ignore storage failures; in-memory interest still works this session.
    }
  }

  void clearObservedUnreadForChannel(String channelId) {
    _latestObservedByChannel.remove(channelId);
    _observedUnreadEventsByChannel.remove(channelId);
    state = state.whenData((channels) => List<Channel>.of(channels));
  }

  void clearObservedUnreadCoveredByRead(String channelId, int readAt) {
    final latest = _latestObservedByChannel[channelId];
    if (latest != null && latest <= readAt) {
      clearObservedUnreadForChannel(channelId);
    }
  }

  /// Backstop refresh that preserves existing state on transient failure.
  Future<void> _backstopRefresh() async {
    try {
      final sessionState = ref.read(relaySessionProvider);
      final prevChannels = state.value ?? const [];
      final prevLastMessage = {
        for (final c in prevChannels)
          if (c.lastMessageAt != null) c.id: c.lastMessageAt,
      };
      final channels = await _fetch(
        subscribeLive: sessionState.status == SessionStatus.connected,
        fetchLastMessage: false,
        fetchDirectory: false,
      );
      for (var i = 0; i < channels.length; i++) {
        final prev = prevLastMessage[channels[i].id];
        if (channels[i].lastMessageAt == null && prev != null) {
          channels[i] = channels[i].copyWith(lastMessageAt: prev);
        }
      }
      state = AsyncData(channels);
    } on _StaleChannelRefresh {
      return;
    } catch (error) {
      debugPrint('[ChannelsNotifier] backstop refresh failed: $error');
      if (_listSnapshot.shown) _recovery.failed(error);
    }
  }

  /// Refreshes memberships and, when [fetchDirectory], the open directory.
  /// Invite starter recovery opts in to converge on another identity's
  /// starters instead of attempting duplicate creation from memberships.
  Future<void> refresh({bool fetchDirectory = false}) async {
    final sessionState = ref.read(relaySessionProvider);
    // Don't attempt to fetch when the session isn't connected — fetchHistory
    // would send REQs over an unauthenticated socket that either time out
    // (returning empty results) or get cancelled on disconnect, replacing the
    // cached channel list with [] or an error. Wait for `build()` to re-run
    // when the session transitions to connected.
    if (sessionState.status != SessionStatus.connected) return;
    try {
      final channels = await _fetch(
        subscribeLive: true,
        fetchDirectory: fetchDirectory,
      );
      state = AsyncData(channels);
    } on _StaleChannelRefresh {
      return;
    } catch (error, stackTrace) {
      if (_listSnapshot.shown) return _recovery.failed(error);
      state = AsyncError(error, stackTrace);
    }
  }

  /// Loads the directory when Browse channels opens after startup or an error.
  Future<void> ensureDirectoryLoaded() async {
    final directoryState = ref.read(channelDirectoryLoadStatusProvider);
    final scope = channelDirectoryScope(
      ref.read(relayConfigProvider).baseUrl,
      ref.read(myPubkeyProvider),
    );
    if (directoryState.scope == scope &&
        (directoryState.status == ChannelDirectoryLoadStatus.loading ||
            directoryState.status == ChannelDirectoryLoadStatus.loaded)) {
      return;
    }
    await retryDirectory();
  }

  /// Retries channel discovery while retaining the current channel list.
  Future<void> retryDirectory() async {
    final previousChannels = state.value;
    final directoryStatus = ref.read(
      channelDirectoryLoadStatusProvider.notifier,
    );
    final scope = channelDirectoryScope(
      ref.read(relayConfigProvider).baseUrl,
      ref.read(myPubkeyProvider),
    );
    final directoryState = ref.read(channelDirectoryLoadStatusProvider);
    if (directoryState.scope == scope &&
        directoryState.status == ChannelDirectoryLoadStatus.loading) {
      return;
    }
    if (ref.read(relaySessionProvider).status != SessionStatus.connected) {
      directoryStatus.markError(scope);
      return;
    }
    try {
      state = AsyncData(
        await _fetch(subscribeLive: true, fetchDirectory: true),
      );
    } on _StaleChannelRefresh {
      // A community or identity switch retired this request. Its response
      // describes a scope the user has left, so write neither the channel list
      // nor the load status; the new scope owns both now.
      return;
    } catch (error, stackTrace) {
      directoryStatus.markError(scope);
      state = previousChannels == null
          ? AsyncError(error, stackTrace)
          : AsyncData(previousChannels);
    }
  }
}

final channelsProvider = AsyncNotifierProvider<ChannelsNotifier, List<Channel>>(
  ChannelsNotifier.new,
);

/// Selects room [id] as the list holds it, or null. A live event in another
/// room keeps this room's [Channel] object, so a screen watching its own room
/// does not rebuild for other rooms' traffic.
Channel? Function(AsyncValue<List<Channel>>) selectChannel(String id) =>
    (channels) => channels.value?.where((c) => c.id == id).firstOrNull;

/// Lower-cased room name → id for `#name` links, compared by content: a live
/// event that only moves some room's recency does not rebuild its watchers.
@immutable
class ChannelNameIds {
  ChannelNameIds.of(AsyncValue<List<Channel>> channels)
    : ids = {
        for (final channel in channels.value ?? const <Channel>[])
          channel.name.toLowerCase(): channel.id,
      };

  final Map<String, String> ids;

  @override
  bool operator ==(Object other) =>
      other is ChannelNameIds && mapEquals(ids, other.ids);

  @override
  int get hashCode => Object.hashAllUnordered(
    ids.entries.map((entry) => Object.hash(entry.key, entry.value)),
  );
}
