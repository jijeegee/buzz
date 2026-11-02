import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/features/channels/thread_root_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

// Opening a thread from its root id
// (docs/message-sync.md#opening-a-thread-from-its-root-id).

const _channel = 'room';
final _root = 'r' * 64;
final _alice = 'a' * 64;
final _mallory = 'm' * 64;

class _ConnectedSession extends RelaySessionNotifier {
  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);
}

NostrEvent _event(String id, int kind, String pubkey, {String? target}) =>
    NostrEvent(
      id: id,
      pubkey: pubkey,
      createdAt: 1,
      kind: kind,
      tags: [
        ['h', _channel],
        if (target != null) ['e', target],
      ],
      content: '',
      sig: '',
    );

Future<void> _settle() async {
  for (var i = 0; i < 10; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

/// Mounts the provider with the relay answering [relay] at once and the
/// store answering only when the returned completer completes.
({
  ProviderContainer container,
  Completer<({NostrEvent event, bool deleted})?> stored,
})
_mount(List<NostrEvent> relay) {
  final stored = Completer<({NostrEvent event, bool deleted})?>();
  final container = ProviderContainer(
    overrides: [
      relaySessionProvider.overrideWith(_ConnectedSession.new),
      threadRootSourcesProvider.overrideWithValue(
        ThreadRootSources(
          stored: (_, _) => stored.future,
          relay: (_, _) async => relay,
        ),
      ),
    ],
  );
  addTearDown(container.dispose);
  return (container: container, stored: stored);
}

final _provider = threadRootProvider(
  ThreadRepliesArgs(channelId: _channel, rootId: _root),
);

void main() {
  test("a lone author's deletion answered before the stored root counts once "
      'the store answers', () async {
    final mounted = _mount([_event('d' * 64, 5, _alice, target: _root)]);
    final sub = mounted.container.listen(_provider, (_, _) {});
    addTearDown(sub.close);
    await _settle();
    expect(mounted.container.read(_provider).deleted, isFalse);

    // The store never saw the deletion (it happened while offline).
    mounted.stored.complete((event: _event(_root, 9, _alice), deleted: false));
    await _settle();
    final root = mounted.container.read(_provider);
    expect(root.event?.id, _root);
    expect(root.deleted, isTrue);
  });

  test("a lone deletion by another author stays unresolved once the author is "
      'known', () async {
    final mounted = _mount([_event('d' * 64, 5, _mallory, target: _root)]);
    final sub = mounted.container.listen(_provider, (_, _) {});
    addTearDown(sub.close);
    await _settle();
    mounted.stored.complete((event: _event(_root, 9, _alice), deleted: false));
    await _settle();
    final root = mounted.container.read(_provider);
    expect(root.event?.id, _root);
    expect(root.deleted, isFalse);
  });
}
