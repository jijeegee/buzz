import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'package:kuzz/features/channels/channel_preload.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/features/channels/thread_saved_replies.dart';
import 'package:kuzz/shared/push/push_inbox_events.dart';
import 'package:kuzz/shared/relay/relay.dart';

import 'window_relay_fixture.dart';

const _channel = '11111111-1111-4111-8111-111111111111';
final _root = 'f' * 64;
final _args = ThreadRepliesArgs(channelId: _channel, rootId: _root);

class _Offline extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
}

NostrEvent _reply(String id, int at, {String content = ''}) => NostrEvent(
  id: id,
  pubkey: 'b' * 64,
  createdAt: at,
  kind: 9,
  tags: [
    ['h', _channel],
    ['e', _root, '', 'reply'],
  ],
  content: content.isEmpty ? id : content,
  sig: '',
);

/// A thread's saved replies read the store again after a push inbox drain
/// wrote to it (docs/message-sync.md#push-inbox).
void main() {
  test('a drain re-reads the stored replies, merged with the preload, '
      'without showing loading again', () async {
    var stored = [_reply('r1', 10)];
    var reads = 0;
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(_Offline.new),
        paintableThreadHeadProvider.overrideWithValue((_, _) async {
          reads++;
          return serveThreadWindow(
            _channel,
            _root,
            stored,
            NostrFilter(kinds: const [], limit: 50),
          );
        }),
        preloadedThreadRepliesProvider.overrideWith(
          (ref, args) => [_reply('r1', 10, content: 'preloaded')],
        ),
      ],
    );
    addTearDown(container.dispose);
    final loading = <bool>[];
    container.listen(
      savedRepliesLoadingProvider(_args),
      (_, next) => loading.add(next),
      fireImmediately: true,
    );
    container.listen(savedThreadRepliesProvider(_args), (_, _) {});
    await container.read(storedThreadRepliesProvider(_args).future);
    expect(
      container.read(savedThreadRepliesProvider(_args)).single.content,
      'preloaded',
    );

    stored = [_reply('r1', 10), _reply('r2', 20)];
    container.read(pushInboxRevisionProvider.notifier).bump();
    await container.read(storedThreadRepliesProvider(_args).future);
    await pumpEventQueue();

    expect(reads, 2);
    final replies = container.read(savedThreadRepliesProvider(_args));
    expect(replies.map((reply) => reply.id), ['r1', 'r2']);
    expect(replies.first.content, 'preloaded', reason: 'the preload wins');
    expect(loading, [true, false], reason: 'the re-read keeps its replies');
  });
}
