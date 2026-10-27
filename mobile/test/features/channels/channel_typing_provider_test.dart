import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_typing_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

void main() {
  const room = '11111111-1111-4111-8111-111111111111';

  ProviderContainer containerWith(_TypingRelaySession session) {
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => session)],
    );
    addTearDown(container.dispose);
    return container;
  }

  test('leaving the room closes its typing REQ', () async {
    final session = _TypingRelaySession();
    final container = containerWith(session);
    final watch = container.listen(channelTypingProvider(room), (_, _) {});

    session.open.single.complete();
    await Future<void>.delayed(Duration.zero);
    expect(session.filters.single.tags['#h'], [room]);

    session.emit(room);
    expect(container.read(channelTypingProvider(room)), hasLength(1));

    watch.close();
    await container.pump();
    expect(session.closed, 1);
  });

  test('a REQ that opens after leaving is closed at once', () async {
    final session = _TypingRelaySession();
    final container = containerWith(session);
    container.listen(channelTypingProvider(room), (_, _) {}).close();
    await container.pump();

    session.open.single.complete();
    await Future<void>.delayed(Duration.zero);
    expect(session.closed, 1);
  });
}

class _TypingRelaySession extends RelaySessionNotifier {
  final filters = <NostrFilter>[];
  final open = <Completer<void>>[];
  final _listeners = <void Function(NostrEvent)>[];
  var closed = 0;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    filters.add(filter);
    final opened = Completer<void>();
    open.add(opened);
    await opened.future;
    _listeners.add(onEvent);
    return () {
      closed++;
      _listeners.remove(onEvent);
    };
  }

  void emit(String room) {
    final event = NostrEvent(
      id: 'typing',
      pubkey: 'alice',
      createdAt: 1,
      kind: EventKind.typingIndicator,
      tags: [
        ['h', room],
      ],
      content: '',
      sig: 'sig',
    );
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}
