import 'dart:async';

import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../channels/failed_local_messages_provider.dart';
import '../channels/pending_local_messages_provider.dart';
import 'forum_models.dart';
import 'forum_provider.dart';

bool _isForumSend(NostrEvent event) =>
    event.kind == EventKind.forumPost || event.kind == EventKind.forumComment;

/// Own forum sends the relay has accepted, kept on screen until the forum
/// list or thread fetched after the acceptance has answered.
///
/// A send waits in [pendingLocalMessagesProvider] (clock, or "Not sent" for
/// retry and delete) like a channel message. The forum lists, though, are
/// relay queries rather than a live timeline: once the relay accepts a send,
/// or echoes it, its pending entry goes and the row would vanish until a
/// refetch includes it. Every forum send leaving pending without having
/// failed is held here and its list refetched; the hold ends when that
/// refetch settles. A failed send leaves only by delete or by a re-signed
/// retry (which adds its own new entry), so it is not held.
class ForumAcceptedSendsNotifier extends Notifier<Map<String, NostrEvent>> {
  final String channelId;

  /// Failed sends still pending; a send leaving pending from here was
  /// deleted or replaced, not accepted.
  final _failed = <String>{};

  ForumAcceptedSendsNotifier(this.channelId);

  @override
  Map<String, NostrEvent> build() {
    ref.listen(failedLocalMessagesProvider(channelId), (_, failed) {
      final pending = ref.read(pendingLocalMessagesProvider(channelId));
      // A mark cleared while the send is still pending is a retry. A mark
      // dropped with its pending entry is settled by the listener below,
      // whichever of the two runs first.
      _failed
        ..removeWhere((id) => !failed.contains(id) && pending.containsKey(id))
        ..addAll(failed);
    }, fireImmediately: true);
    ref.listen(pendingLocalMessagesProvider(channelId), (previous, next) {
      for (final event in previous?.values ?? const <NostrEvent>[]) {
        if (next.containsKey(event.id)) continue;
        if (_failed.remove(event.id)) continue;
        if (_isForumSend(event)) _hold(event);
      }
    });
    return const {};
  }

  void _hold(NostrEvent event) {
    state = {...state, event.id: event};
    // The posts list is refreshed for replies too, as a reply always did.
    final posts = forumPostsProvider(channelId);
    ref.invalidate(posts);
    final parentId = event.kind == EventKind.forumComment
        ? event.threadReference.parentId
        : null;
    final Future<Object?> refetch;
    if (parentId == null) {
      refetch = ref.read(posts.future);
    } else {
      final thread = forumThreadProvider((
        channelId: channelId,
        eventId: parentId,
      ));
      ref.invalidate(thread);
      refetch = ref.read(thread.future);
    }
    // A failed refetch shows its own error; the hold never outlives it.
    unawaited(
      refetch.then<void>((_) {}, onError: (_) {}).whenComplete(() {
        if (!ref.mounted || !state.containsKey(event.id)) return;
        state = {...state}..remove(event.id);
      }),
    );
  }
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
