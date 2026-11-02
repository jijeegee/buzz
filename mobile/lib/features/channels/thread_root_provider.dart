import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'channel_messages_provider.dart';
import 'thread_replies_provider.dart';

/// What a thread page opened by its root id knows of the root
/// (docs/message-sync.md#opening-a-thread-from-its-root-id). The page draws
/// without it: the root only fills the top bubble, agent pins and the menu.
@immutable
class ThreadRoot {
  const ThreadRoot({this.event, this.deleted = false});

  /// The root as found so far: on the timeline, in the store, then from the
  /// relay, whose copy replaces a stored one.
  final NostrEvent? event;

  /// Only on an explicit deletion: a stored or relay removal (9005) or the
  /// author's deletion (5). A failed or empty lookup never means deleted.
  final bool deleted;
}

/// What one relay answer for a root says. Cases:
/// test-fixtures/thread-open-root.json.
enum ThreadRootVerdict {
  /// The root came back.
  found,

  /// A deletion of the root came back.
  deleted,

  /// Neither: not known yet, asked again after a reconnect. A timeout, a
  /// refusal or a dropped connection is this too.
  unresolved,
}

/// The verdict of [answer] (null when the request failed) to
/// [threadRootFilters] for [rootId] in [channelId], and the root if found.
({ThreadRootVerdict verdict, NostrEvent? root}) threadRootVerdict(
  List<NostrEvent>? answer, {
  required String channelId,
  required String rootId,
}) {
  if (answer == null) {
    return (verdict: ThreadRootVerdict.unresolved, root: null);
  }
  final root = answer
      .where((event) => event.id == rootId && event.channelId == channelId)
      .firstOrNull;
  final deleted = answer.any(
    (event) =>
        event.channelId == channelId &&
        (event.kind == EventKind.nip29DeleteEvent ||
            (event.kind == EventKind.deletion &&
                (root == null || event.pubkey == root.pubkey))) &&
        event.tags.any(
          (tag) => tag.length > 1 && tag[0] == 'e' && tag[1] == rootId,
        ),
  );
  if (deleted) return (verdict: ThreadRootVerdict.deleted, root: root);
  if (root != null) return (verdict: ThreadRootVerdict.found, root: root);
  return (verdict: ThreadRootVerdict.unresolved, root: null);
}

/// One `/query` for a root: the root by id, and a deletion of it.
List<NostrFilter> threadRootFilters(String channelId, String rootId) => [
  NostrFilter(
    kinds: EventKind.channelTimelineContentKinds,
    ids: [rootId],
    limit: 1,
  ),
  NostrFilter(
    kinds: const [EventKind.deletion, EventKind.nip29DeleteEvent],
    tags: {
      '#h': [channelId],
      '#e': [rootId],
    },
    limit: 1,
  ),
];

/// Where a root is looked up; overridden in tests.
@immutable
class ThreadRootSources {
  const ThreadRootSources({required this.stored, required this.relay});

  final Future<({NostrEvent event, bool deleted})?> Function(
    RelaySessionNotifier session,
    ThreadRepliesArgs args,
  )
  stored;
  final Future<List<NostrEvent>> Function(
    RelaySessionNotifier session,
    List<NostrFilter> filters,
  )
  relay;
}

final threadRootSourcesProvider = Provider<ThreadRootSources>(
  (ref) => ThreadRootSources(
    stored: (session, args) =>
        session.paintableEvent(args.channelId, args.rootId),
    relay: (session, filters) => session.queryRelay(filters),
  ),
);

/// Resolves a thread's root: the channel's loaded rows, then the store by
/// id, then the relay (over HTTP, so a dropped socket does not stop it),
/// asked again whenever the session reconnects until it answers.
class ThreadRootNotifier extends Notifier<ThreadRoot> {
  ThreadRootNotifier(this.args);

  final ThreadRepliesArgs args;
  bool _asking = false;
  bool _answered = false;

  @override
  ThreadRoot build() {
    ref.listen(relaySessionProvider, (previous, next) {
      if (previous?.status != SessionStatus.connected &&
          next.status == SessionStatus.connected) {
        unawaited(_askRelay());
      }
    });
    scheduleMicrotask(() {
      if (!ref.mounted) return;
      unawaited(_readStore());
      unawaited(_askRelay());
    });
    return ThreadRoot(event: _fromTimeline());
  }

  NostrEvent? _fromTimeline() {
    final channel = channelMessagesProvider(args.channelId);
    if (!ref.exists(channel)) return null;
    return ref
            .read(channel)
            .value
            ?.where((event) => event.id == args.rootId)
            .firstOrNull ??
        ref.read(channel.notifier).deepLinkEvent(args.rootId);
  }

  Future<void> _readStore() async {
    final session = ref.read(relaySessionProvider.notifier);
    final ({NostrEvent event, bool deleted})? stored;
    try {
      stored = await ref.read(threadRootSourcesProvider).stored(session, args);
    } catch (error) {
      debugPrint('[ThreadRoot] stored root unavailable: $error');
      return;
    }
    // The relay's answer wins over a late stored one.
    if (stored == null || !ref.mounted || _answered) return;
    if (stored.event.channelId != args.channelId) return;
    state = ThreadRoot(
      event: state.event ?? stored.event,
      deleted: state.deleted || stored.deleted,
    );
  }

  Future<void> _askRelay() async {
    if (_asking || _answered) return;
    _asking = true;
    List<NostrEvent>? answer;
    try {
      answer = await ref
          .read(threadRootSourcesProvider)
          .relay(
            ref.read(relaySessionProvider.notifier),
            threadRootFilters(args.channelId, args.rootId),
          );
    } catch (error) {
      debugPrint('[ThreadRoot] relay root unavailable: $error');
    } finally {
      _asking = false;
    }
    if (!ref.mounted) return;
    final result = threadRootVerdict(
      answer,
      channelId: args.channelId,
      rootId: args.rootId,
    );
    switch (result.verdict) {
      case ThreadRootVerdict.found:
        _answered = true;
        state = ThreadRoot(event: result.root, deleted: state.deleted);
      case ThreadRootVerdict.deleted:
        _answered = true;
        state = ThreadRoot(event: result.root ?? state.event, deleted: true);
      case ThreadRootVerdict.unresolved:
        break;
    }
  }
}

final threadRootProvider = NotifierProvider.autoDispose
    .family<ThreadRootNotifier, ThreadRoot, ThreadRepliesArgs>(
      ThreadRootNotifier.new,
    );
