import 'dart:async';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/misc.dart' show KeepAliveLink;

import '../../shared/relay/relay.dart';
import 'channel_event_order.dart';
import 'channel_messages_provider.dart';
import 'pending_local_messages_provider.dart';

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
      ref.onCancel(() {
        listened = false;
        releaseTimer?.cancel();
        releaseTimer = Timer(threadRepliesRetention, retention.close);
      });
      ref.onResume(() {
        listened = true;
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
      ref.onDispose(() => releaseTimer?.cancel());
      ref.listen(threadRepliesStaleProvider(args), (_, _) => refresh());
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
      try {
        final replies = await fetchCompleteThreadReplies(session, args);
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
      final relayReplies = ref.watch(threadRepliesProvider(args));
      final localReplies = ref.watch(threadLocalRepliesProvider(args));
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
