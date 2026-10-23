import 'dart:async';

import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../channels/pending_local_messages_provider.dart';
import 'forum_models.dart';
import 'forum_provider.dart';

bool _isForumSend(NostrEvent event) =>
    event.kind == EventKind.forumPost || event.kind == EventKind.forumComment;

/// The post a forum send is listed under: null for a post (the posts list),
/// else the thread root a reply answers.
String? _listRootOf(NostrEvent event) => event.kind == EventKind.forumComment
    ? event.threadReference.parentId
    : null;

/// Own forum sends the relay has accepted, kept on screen until a fetched
/// forum list or thread contains them.
///
/// A send waits in [pendingLocalMessagesProvider] (clock, or "Not sent" for
/// retry and delete) like a channel message. The forum lists, though, are
/// relay queries rather than a live timeline: once the relay accepts a send,
/// or echoes it, its pending entry goes and the row would vanish until a
/// refetch includes it. Every forum send leaving pending other than by being
/// dropped (delete, or a re-signed retry replacing it) is held here while a
/// loaded list that lacks it refetches. The hold ends when a fetched list
/// contains it, or when that refetch succeeds without it (the relay no longer
/// lists it); a failed refetch keeps it.
///
/// Fetched lists also confirm the pending sends they contain, so a send the
/// relay stored but never acknowledged stops showing "Not sent".
class ForumAcceptedSendsNotifier extends Notifier<Map<String, NostrEvent>> {
  final String channelId;

  /// Lists to refetch at the end of this microtask, keyed by
  /// [_listRootOf], each with the holds its refetch answers.
  Map<String?, Set<String>>? _refetches;

  ForumAcceptedSendsNotifier(this.channelId);

  @override
  Map<String, NostrEvent> build() {
    final pendingProvider = pendingLocalMessagesProvider(channelId);
    ref.listen(pendingProvider, (previous, next) {
      final pending = ref.read(pendingProvider.notifier);
      for (final event in previous?.values ?? const <NostrEvent>[]) {
        if (next.containsKey(event.id) || pending.wasDropped(event.id)) {
          continue;
        }
        if (_isForumSend(event)) _hold(event);
      }
    });
    return const {};
  }

  /// Takes in a forum list or thread the relay just returned: its pending
  /// sends are confirmed (failed mark and stored copy included), and its
  /// held sends are released.
  void confirmFetched(Iterable<String> eventIds) {
    final ids = eventIds.toSet();
    ref.read(pendingLocalMessagesProvider(channelId).notifier).confirm(ids);
    release(ids);
  }

  /// Ends the holds on [eventIds], e.g. once they are deleted.
  void release(Set<String> eventIds) {
    if (!state.keys.any(eventIds.contains)) return;
    state = {
      for (final entry in state.entries)
        if (!eventIds.contains(entry.key)) entry.key: entry.value,
    };
  }

  void _hold(NostrEvent event) {
    final rootId = _listRootOf(event);
    final listed = _loadedIds(rootId);
    if (listed?.contains(event.id) ?? false) return;
    // The posts list is refreshed for replies too, as a reply always did.
    if (rootId != null) _queueRefetch(null);
    // A list nothing has loaded fetches the send when it is opened.
    if (listed == null) return;
    state = {...state, event.id: event};
    _queueRefetch(rootId).add(event.id);
  }

  /// Ids in the loaded list for [rootId], or null when it isn't loaded.
  Set<String>? _loadedIds(String? rootId) {
    if (rootId == null) {
      final posts = forumPostsProvider(channelId);
      if (!ref.exists(posts)) return null;
      return {...?ref.read(posts).value?.posts.map((post) => post.eventId)};
    }
    final thread = _thread(rootId);
    if (!ref.exists(thread)) return null;
    return {...?ref.read(thread).value?.replies.map((reply) => reply.eventId)};
  }

  /// The holds [rootId]'s refetch answers; N sends leaving in one go cause
  /// one refetch per list.
  Set<String> _queueRefetch(String? rootId) {
    var refetches = _refetches;
    if (refetches == null) {
      refetches = _refetches = {};
      scheduleMicrotask(_refetch);
    }
    return refetches.putIfAbsent(rootId, () => {});
  }

  void _refetch() {
    final refetches = _refetches;
    _refetches = null;
    if (refetches == null || !ref.mounted) return;
    for (final MapEntry(key: rootId, value: held) in refetches.entries) {
      final Future<Iterable<String>> fetched;
      if (rootId == null) {
        final posts = forumPostsProvider(channelId);
        if (!ref.exists(posts)) continue;
        ref.invalidate(posts);
        fetched = ref
            .read(posts.future)
            .then((response) => response.posts.map((post) => post.eventId));
      } else {
        final thread = _thread(rootId);
        if (!ref.exists(thread)) continue;
        ref.invalidate(thread);
        fetched = ref
            .read(thread.future)
            .then(
              (response) => [
                response.post.eventId,
                for (final reply in response.replies) reply.eventId,
              ],
            );
      }
      // A failed refetch shows its own error and keeps the holds.
      unawaited(
        fetched.then<void>((ids) {
          if (!ref.mounted) return;
          confirmFetched(ids);
          release(held);
        }, onError: (_) {}),
      );
    }
  }

  FutureProvider<ForumThreadResponse> _thread(String rootId) =>
      forumThreadProvider((channelId: channelId, eventId: rootId));
}

final forumAcceptedSendsProvider =
    NotifierProvider.family<
      ForumAcceptedSendsNotifier,
      Map<String, NostrEvent>,
      String
    >(ForumAcceptedSendsNotifier.new);

/// [signer]'s forum sends in [channelId] that a fetched forum list may not
/// include yet: sending, failed, or accepted and awaiting its refetch.
List<NostrEvent> watchOwnForumSends(
  WidgetRef ref,
  String channelId,
  String? signer,
) {
  final pending = ref.watch(pendingLocalMessagesProvider(channelId));
  final accepted = ref.watch(forumAcceptedSendsProvider(channelId));
  if (pending.isEmpty && accepted.isEmpty) return const [];
  return [
    for (final event in splitLocalSendsBySigner({
      ...accepted,
      ...pending,
    }, signer).own)
      if (_isForumSend(event)) event,
  ];
}

/// [fetched] (newest first) with the [local] posts it lacks, in place.
List<ForumPost> withLocalForumPosts(
  List<ForumPost> fetched,
  List<NostrEvent> local,
) {
  final known = {for (final post in fetched) post.eventId};
  final added = [
    for (final event in local)
      if (event.kind == EventKind.forumPost && known.add(event.id))
        ForumPost.fromEvent(event),
  ];
  if (added.isEmpty) return fetched;
  final merged = List.of(fetched);
  for (final post in added) {
    final index = merged.indexWhere(
      (other) => other.createdAt <= post.createdAt,
    );
    merged.insert(index < 0 ? merged.length : index, post);
  }
  return merged;
}

/// [fetched] (oldest first) with the [local] replies to [postId] it lacks,
/// in place.
List<ThreadReply> withLocalForumReplies(
  List<ThreadReply> fetched,
  List<NostrEvent> local,
  String postId,
) {
  final known = {for (final reply in fetched) reply.eventId};
  final added = [
    for (final event in local)
      if (event.kind == EventKind.forumComment &&
          event.threadReference.parentId == postId &&
          known.add(event.id))
        ThreadReply.fromEvent(event),
  ];
  if (added.isEmpty) return fetched;
  final merged = List.of(fetched);
  for (final reply in added) {
    final index = merged.indexWhere(
      (other) => other.createdAt > reply.createdAt,
    );
    merged.insert(index < 0 ? merged.length : index, reply);
  }
  return merged;
}
