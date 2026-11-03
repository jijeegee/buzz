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
/// An author's deletion (5) counts only when its signer is the root's author,
/// so the root must be in the answer or already known as [knownRoot].
({ThreadRootVerdict verdict, NostrEvent? root}) threadRootVerdict(
  List<NostrEvent>? answer, {
  required String channelId,
  required String rootId,
  NostrEvent? knownRoot,
}) {
  if (answer == null) {
    return (verdict: ThreadRootVerdict.unresolved, root: null);
  }
  final root = answer
      .where((event) => event.id == rootId && event.channelId == channelId)
      .firstOrNull;
  final author = (root ?? (knownRoot?.id == rootId ? knownRoot : null))?.pubkey;
  final deleted = answer.any(
    (event) =>
        event.channelId == channelId &&
        (event.kind == EventKind.nip29DeleteEvent ||
            (event.kind == EventKind.deletion &&
                author != null &&
                event.pubkey == author)) &&
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
  NostrFilter(kinds: EventKind.channelTimelineContentKinds, ids: [rootId]),
  NostrFilter(
    kinds: const [EventKind.deletion, EventKind.nip29DeleteEvent],
    tags: {
      '#h': [channelId],
      '#e': [rootId],
    },
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
/// asked again whenever the session reconnects until it answers. A failed
/// request while connected is retried after [retryDelays], then waits for
/// the next reconnect. An answer holding only an author's deletion (5) while
/// the root's author is unknown is kept and judged again once the store
/// answers with the root.
class ThreadRootNotifier extends Notifier<ThreadRoot> {
  ThreadRootNotifier(this.args);

  static const retryDelays = [
    Duration(seconds: 2),
    Duration(seconds: 5),
    Duration(seconds: 15),
  ];

  final ThreadRepliesArgs args;
  bool _asking = false;
  bool _answered = false;
  int _failures = 0;
  Timer? _retry;

  /// The last answer holding an author's deletion that could not be judged
  /// because the root's author was not known yet.
  List<NostrEvent>? _pending;

  @override
  ThreadRoot build() {
    ref.onDispose(() => _retry?.cancel());
    ref.listen(relaySessionProvider, (previous, next) {
      if (previous?.status != SessionStatus.connected &&
          next.status == SessionStatus.connected) {
        _failures = 0;
        _retry?.cancel();
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
    if (_pending case final kept?) _judge(kept);
  }

  Future<void> _askRelay() async {
    if (_asking || _answered) return;
    _asking = true;
    _retry?.cancel();
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
    if (answer == null) {
      _scheduleRetry();
      return;
    }
    _judge(answer);
  }

  void _judge(List<NostrEvent> answer) {
    final result = threadRootVerdict(
      answer,
      channelId: args.channelId,
      rootId: args.rootId,
      knownRoot: state.event ?? _fromTimeline(),
    );
    switch (result.verdict) {
      case ThreadRootVerdict.found:
        _answered = true;
        _pending = null;
        state = ThreadRoot(event: result.root, deleted: state.deleted);
      case ThreadRootVerdict.deleted:
        _answered = true;
        _pending = null;
        state = ThreadRoot(event: result.root ?? state.event, deleted: true);
      case ThreadRootVerdict.unresolved:
        _pending = answer.any((event) => event.kind == EventKind.deletion)
            ? answer
            : null;
    }
  }

  void _scheduleRetry() {
    if (_failures >= retryDelays.length) return;
    if (ref.read(relaySessionProvider).status != SessionStatus.connected) {
      return;
    }
    _retry = Timer(retryDelays[_failures++], () {
      if (ref.mounted) unawaited(_askRelay());
    });
  }
}

final threadRootProvider = NotifierProvider.autoDispose
    .family<ThreadRootNotifier, ThreadRoot, ThreadRepliesArgs>(
      ThreadRootNotifier.new,
    );
