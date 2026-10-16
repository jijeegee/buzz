import 'dart:async';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/misc.dart' show KeepAliveLink;

import '../../shared/relay/relay.dart';
import 'channel_event_order.dart';
import 'channel_messages_provider.dart';
import 'pending_local_messages_provider.dart';
import 'thread_window.dart';

class ThreadRepliesArgs {
  final String channelId;
  final String rootId;

  const ThreadRepliesArgs({required this.channelId, required this.rootId});

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is ThreadRepliesArgs &&
          channelId == other.channelId &&
          rootId == other.rootId;

  @override
  int get hashCode => Object.hash(channelId, rootId);
}

class _ThreadCursor {
  final int createdAt;
  final String eventId;

  const _ThreadCursor({required this.createdAt, required this.eventId});
}

/// How long a closed thread keeps its replies in memory. Reopening within this
/// window shows them at once; a reply or reconnect seen while it was closed
/// marks it stale, and it refreshes in the background when reopened.
const threadRepliesRetention = Duration(hours: 12);

/// Most closed threads kept at once; the longest-closed is released first.
const maxRetainedClosedThreads = 32;

/// Bounds how many closed threads keep their replies: each closed thread
/// registers its release, and registering past [maxRetainedClosedThreads]
/// releases the longest-closed one.
class ClosedThreadRetention {
  ClosedThreadRetention({this.maxThreads = maxRetainedClosedThreads});

  final int maxThreads;
  final Map<Object, void Function()> _releases = {};

  int get length => _releases.length;

  void add(Object thread, void Function() release) {
    _releases.remove(thread);
    _releases[thread] = release;
    while (_releases.length > maxThreads) {
      final oldest = _releases.keys.first;
      final evict = _releases.remove(oldest)!;
      // Lifecycle callbacks may not touch providers; release right after.
      scheduleMicrotask(evict);
    }
  }

  void remove(Object thread) => _releases.remove(thread);
}

final closedThreadRetentionProvider = Provider<ClosedThreadRetention>(
  (ref) => ClosedThreadRetention(),
);

/// Bumped when a thread's replies may be out of date (a live reply arrived, or
/// the session reconnected). The thread query watches it, so an open thread
/// refreshes at once and a closed, retained one when it is next listened to.
///
/// Unlike `ref.invalidate`, a dependency change does not dispose a retained
/// query that nobody is listening to, so its replies stay readable.
class ThreadRepliesStaleNotifier extends Notifier<int> {
  final ThreadRepliesArgs args;

  ThreadRepliesStaleNotifier(this.args);

  @override
  int build() => 0;

  void markStale() => state++;
}

final threadRepliesStaleProvider = NotifierProvider.autoDispose
    .family<ThreadRepliesStaleNotifier, int, ThreadRepliesArgs>(
      ThreadRepliesStaleNotifier.new,
    );

final threadRepliesProvider = FutureProvider.autoDispose
    .family<List<NostrEvent>, ThreadRepliesArgs>((ref, args) async {
      // Closing the thread page releases the last listener. Keep the replies
      // for [threadRepliesRetention] instead of dropping them at once.
      //
      // Rebuilding a query nobody listens to disposes it (and its replies), so
      // a stale mark while closed (a live reply, a reconnect) is held until the
      // thread is reopened, then refreshed while the kept replies still show.
      final retention = ref.keepAlive();
      var listened = true;
      var stale = false;
      void refresh() {
        if (listened) {
          ref.invalidateSelf();
        } else {
          stale = true;
        }
      }

      Timer? releaseTimer;
      final closedThreads = ref.read(closedThreadRetentionProvider);
      final retentionKey = Object();
      void release() {
        releaseTimer?.cancel();
        releaseTimer = null;
        closedThreads.remove(retentionKey);
        retention.close();
      }

      ref.onCancel(() {
        listened = false;
        releaseTimer?.cancel();
        releaseTimer = Timer(threadRepliesRetention, release);
        closedThreads.add(retentionKey, () {
          // Reopened since this release was scheduled: keep it.
          if (!listened) release();
        });
      });
      ref.onResume(() {
        listened = true;
        closedThreads.remove(retentionKey);
        releaseTimer?.cancel();
        releaseTimer = null;
        // Lifecycle callbacks may not touch providers; decide right after.
        scheduleMicrotask(() {
          if (!ref.mounted || !listened) return;
          // Sends still awaiting proof (ACK or deletion) are settled only by
          // a scan, so a thread holding them rescans on reopen as before.
          final channel = channelMessagesProvider(args.channelId);
          final awaitingProof =
              ref.exists(channel) &&
              ref
                  .read(channel.notifier)
                  .unconfirmedThreadReplyIds(args.rootId)
                  .isNotEmpty;
          if (stale || awaitingProof) {
            stale = false;
            ref.invalidateSelf();
          }
        });
      });
      ref.onDispose(() {
        releaseTimer?.cancel();
        closedThreads.remove(retentionKey);
      });
      ref.listen(threadRepliesStaleProvider(args), (_, _) => refresh());
      // The window store lives as long as this query; folding a page into it
      // must not rebuild (and refetch) the query.
      ref.listen(threadWindowProvider(args), (_, _) {});
      // A reply missed while the socket is stale cannot invalidate this
      // one-shot query. Refresh threads when the session recovers.
      ref.listen(relaySessionProvider, (previous, next) {
        if (previous?.status != SessionStatus.connected &&
            next.status == SessionStatus.connected) {
          refresh();
        }
      });
      final session = ref.read(relaySessionProvider.notifier);
      final channelProvider = channelMessagesProvider(args.channelId);
      final channelMessages = ref.exists(channelProvider)
          ? ref.read(channelProvider.notifier)
          : null;
      final cachedReplyIds = channelMessages?.cachedThreadReplyIds(args.rootId);
      final unconfirmedIds = channelMessages?.unconfirmedThreadReplyIds(
        args.rootId,
      );
      final queryVersion = channelMessages?.beginThreadQuery(args.rootId);
      if (queryVersion != null) {
        ref.onDispose(
          () => channelMessages?.failThreadQuery(args.rootId, queryVersion),
        );
      }
      // Newest replies first, like desktop: one window page folded into what
      // this thread already holds; older pages load on scroll-up. Sends still
      // awaiting proof need the complete scan below to settle, and a relay
      // that serves no window falls back to it too.
      final window = ref.read(threadWindowProvider(args).notifier);
      if (unconfirmedIds == null || unconfirmedIds.isEmpty) {
        try {
          final page = parseThreadWindowPage(
            await session.queryRelay([
              threadWindowFilter(
                channelId: args.channelId,
                rootId: args.rootId,
              ),
            ]),
            channelId: args.channelId,
            rootId: args.rootId,
          );
          if (page != null) {
            final folded = window.foldNewest(page);
            if (queryVersion != null) {
              channelMessages?.failThreadQuery(args.rootId, queryVersion);
            }
            if (ref.mounted && ref.exists(channelProvider)) {
              ref
                  .read(channelProvider.notifier)
                  .cacheConfirmedThreadReplies(page.events);
            }
            return folded.events;
          }
        } catch (_) {
          if (queryVersion != null) {
            channelMessages?.failThreadQuery(args.rootId, queryVersion);
          }
          rethrow;
        }
      }
      try {
        final replies = await fetchCompleteThreadReplies(session, args);
        // A complete scan holds the whole thread: nothing older to page.
        window.replaceComplete(replies);
        // Explicit markers also settle unacknowledged local sends, whose
        // absence cannot prove deletion even in an insertion-complete scan.
        final missingIds =
            unconfirmedIds?.difference(
              replies.map((event) => event.id).toSet(),
            ) ??
            <String>{};
        final deletions = <NostrEvent>[];
        // One bounded request per scan keeps opening a thread responsive even
        // when many sends are awaiting ACKs. Excess IDs remain provisional.
        final targets =
            channelMessages?.nextThreadDeletionProofTargets(missingIds) ??
            const <String>[];
        // Per-target limits keep repeated markers from crowding another ID out.
        if (targets.isNotEmpty) {
          deletions.addAll(
            await session.queryRelay([
              for (final target in targets)
                NostrFilter(
                  kinds: const [EventKind.deletion, EventKind.nip29DeleteEvent],
                  tags: {
                    '#h': [args.channelId],
                    '#e': [target],
                  },
                  limit: 1,
                ),
            ]),
          );
        }
        if (ref.mounted && ref.exists(channelProvider)) {
          final channel = ref.read(channelProvider.notifier);
          final deletedTargets = {
            for (final event in deletions)
              for (final tag in event.tags)
                if (tag.length > 1 && tag[0] == 'e') tag[1],
          };
          final applied = channel.cacheCompleteThreadQuery(
            args.rootId,
            cachedReplyIds ?? {},
            replies,
            provisionalReplyIds: missingIds.difference(deletedTargets),
            queryVersion: queryVersion,
          );
          if (deletions.isNotEmpty) {
            channel.cacheThreadDeletions(
              deletions,
              scopedTargetIds: targets.toSet(),
              reconciledTargetIds: applied ? missingIds : const {},
            );
          }
        }
        return replies;
      } catch (_) {
        if (queryVersion != null) {
          channelMessages?.failThreadQuery(args.rootId, queryVersion);
        }
        rethrow;
      }
    });

/// The replies one thread holds in newest-first windows, and how far back
/// they reach. Lives as long as the thread's query (see
/// [threadRepliesProvider]); older pages fold in here without refetching it.
class ThreadWindowNotifier extends Notifier<ThreadWindow?> {
  final ThreadRepliesArgs args;

  ThreadWindowNotifier(this.args);

  bool _loadingOlder = false;

  @override
  ThreadWindow? build() => null;

  bool get hasOlder => state?.state.hasOlder ?? false;

  bool get isLoadingOlder => _loadingOlder;

  ThreadWindow foldNewest(ThreadWindowPage page) =>
      state = mergeNewestThreadPage(state, page);

  void replaceComplete(List<NostrEvent> replies) {
    state = ThreadWindow(
      events: List.of(replies),
      // Covered to its newest row, so a later newest page connects to it
      // instead of restarting from that page.
      state: ThreadWindowState(
        hasOlder: false,
        newestCovered: newestThreadRow(replies),
      ),
    );
  }

  /// Loads the page before the oldest held reply. Returns whether replies
  /// were added.
  Future<bool> loadOlder() async {
    final current = state;
    final cursor = current?.state.olderCursor;
    if (_loadingOlder || current == null || !current.state.hasOlder) {
      return false;
    }
    if (cursor == null) return false;
    _loadingOlder = true;
    try {
      final session = ref.read(relaySessionProvider.notifier);
      final page = parseThreadWindowPage(
        await session.queryRelay([
          threadWindowFilter(
            channelId: args.channelId,
            rootId: args.rootId,
            cursor: cursor,
          ),
        ]),
        channelId: args.channelId,
        rootId: args.rootId,
      );
      if (!ref.mounted || page == null) return false;
      // A newest-page reload may have restarted the window meanwhile; this
      // page then no longer borders what is held.
      if (state?.state.olderCursor != cursor) return false;
      state = mergeOlderThreadPage(state, page);
      return page.events.isNotEmpty;
    } finally {
      _loadingOlder = false;
    }
  }
}

final threadWindowProvider = NotifierProvider.autoDispose
    .family<ThreadWindowNotifier, ThreadWindow?, ThreadRepliesArgs>(
      ThreadWindowNotifier.new,
    );

/// Exhaustively scans a thread using insertion-complete cursor pages.
/// [isCurrent] lets a background refresh stop between pages after disposal.
Future<List<NostrEvent>> fetchCompleteThreadReplies(
  RelaySessionNotifier session,
  ThreadRepliesArgs args, {
  bool Function()? isCurrent,
}) async {
  final replies = <NostrEvent>[];
  // -1 precedes unsigned Nostr timestamps. A non-null cursor selects the
  // insertion-complete route and writer-verified EOF instead of a stale head.
  _ThreadCursor? cursor = const _ThreadCursor(
    createdAt: -1,
    eventId: '0000000000000000000000000000000000000000000000000000000000000000',
  );
  for (var page = 0; page < 500; page++) {
    if (isCurrent != null && !isCurrent()) {
      throw StateError('Thread scan superseded');
    }
    final events = await session.queryRelay([
      _threadRepliesFilter(args, cursor),
    ]);
    replies.addAll(events);
    if (events.length < 200) return replies;
    final last = events.last;
    cursor = _ThreadCursor(createdAt: last.createdAt, eventId: last.id);
  }
  throw Exception('Thread ${args.rootId} exceeded the page safety limit.');
}

NostrFilter _threadRepliesFilter(
  ThreadRepliesArgs args,
  _ThreadCursor? cursor,
) {
  return NostrFilter(
    kinds: EventKind.channelTimelineContentKinds,
    tags: {
      '#e': [args.rootId],
      '#h': [args.channelId],
    },
    limit: 200,
    extensions: {
      // The relay binds this as signed i32. Include every representable depth.
      'depth_limit': 0x7fffffff,
      if (cursor != null) 'thread_cursor': cursor.createdAt,
      if (cursor != null) 'thread_cursor_id': cursor.eventId,
    },
  );
}

class ThreadLocalRepliesNotifier extends Notifier<List<NostrEvent>> {
  final ThreadRepliesArgs args;

  ThreadLocalRepliesNotifier(this.args);

  @override
  List<NostrEvent> build() {
    // Keep optimistic replies across route disposal, but release empty overlays.
    KeepAliveLink? retention;
    listenSelf((previous, next) {
      if (next.isNotEmpty) {
        retention ??= ref.keepAlive();
      } else {
        retention?.close();
        retention = null;
      }
    });
    return const [];
  }

  void add(NostrEvent event) {
    state = _mergeReplies(state, [event]);
  }

  void remove(String eventId) {
    state = state.where((event) => event.id != eventId).toList();
  }

  void confirm(Set<String> eventIds) {
    if (!state.any((event) => eventIds.contains(event.id))) return;
    state = state.where((event) => !eventIds.contains(event.id)).toList();
  }
}

final threadLocalRepliesProvider = NotifierProvider.autoDispose
    .family<ThreadLocalRepliesNotifier, List<NostrEvent>, ThreadRepliesArgs>(
      ThreadLocalRepliesNotifier.new,
    );

/// Relay-backed replies merged with signed local replies that are still
/// waiting for acknowledgement.
///
/// The relay query is route-scoped, while the optimistic local overlay stays
/// alive until confirmation so it can survive closing and reopening a thread.
final threadRepliesWithLocalProvider = Provider.autoDispose
    .family<AsyncValue<List<NostrEvent>>, ThreadRepliesArgs>((ref, args) {
      final queryReplies = ref.watch(threadRepliesProvider(args));
      final localReplies = ref.watch(threadLocalRepliesProvider(args));
      // Older pages fold into the window after the query resolved; show them
      // without refetching the query.
      final windowEvents = ref.watch(threadWindowProvider(args))?.events;
      // Only the displayed value comes from here (status stays on the query),
      // so a refresh keeps showing the held window instead of shrinking it.
      final relayReplies = windowEvents != null && queryReplies.hasValue
          ? AsyncData(windowEvents)
          : queryReplies;
      final authoritative = relayReplies.value;
      if (authoritative != null && localReplies.isNotEmpty) {
        final authoritativeIds = authoritative.map((event) => event.id).toSet();
        if (localReplies.any((event) => authoritativeIds.contains(event.id))) {
          final localRepliesNotifier = ref.read(
            threadLocalRepliesProvider(args).notifier,
          );
          final pendingMessagesNotifier = ref.read(
            pendingLocalMessagesProvider(args.channelId).notifier,
          );
          final channelMessages = ref.read(
            channelMessagesProvider(args.channelId).notifier,
          );
          final confirmedReplies = authoritative
              .where(
                (event) => localReplies.any((local) => local.id == event.id),
              )
              .toList();
          Future.microtask(() {
            channelMessages.cacheConfirmedThreadReplies(confirmedReplies);
            localRepliesNotifier.confirm(authoritativeIds);
            pendingMessagesNotifier.confirm(authoritativeIds);
          });
        }
      }
      if (localReplies.isEmpty) return relayReplies;
      // This provider supplies display events; the original query remains
      // the source of loading/error status. Preserve its retained value when
      // merging optimistic replies during a failed refresh or retry.
      return AsyncData(_mergeReplies(authoritative ?? const [], localReplies));
    });

/// Union two event lists by id, newest-wins, in timeline order.
///
/// The thread view needs this to fold the channel's live socket events into its
/// own one-shot query result: the query asks for content kinds only, so
/// reactions, edits, and deletions that land while a thread is open never reach
/// it on their own.
List<NostrEvent> mergeThreadEvents(
  Iterable<NostrEvent> first,
  Iterable<NostrEvent> second,
) => _mergeReplies(first, second);

List<NostrEvent> _mergeReplies(
  Iterable<NostrEvent> first,
  Iterable<NostrEvent> second,
) {
  final byId = <String, NostrEvent>{};
  for (final event in [...first, ...second]) {
    byId[event.id] = event;
  }
  return byId.values.toList()..sort(compareThreadRepliesChronologically);
}
