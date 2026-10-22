import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/channel_messages_provider.dart';
import 'package:kuzz/features/channels/chat_bubble_row.dart';
import 'package:kuzz/features/channels/failed_local_messages_provider.dart';
import 'package:kuzz/features/channels/local_send_status_mark.dart';
import 'package:kuzz/features/channels/pending_local_messages_provider.dart';
import 'package:kuzz/features/channels/send_message_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _channelId = '11111111-1111-4111-8111-111111111111';

void main() {
  late String nsec;
  late String self;

  setUp(() {
    final keys = nostr.Keys.generate();
    nsec = keys.nsec;
    self = keys.public;
  });

  Future<(ProviderContainer, _ControlledSession)> openChannel({
    Future<List<ChannelMember>> Function(int build)? members,
  }) async {
    final session = _ControlledSession();
    var memberBuilds = 0;
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        relayConfigProvider.overrideWith(() => _FixedConfig(nsec)),
        if (members != null)
          channelMembersProvider(
            _channelId,
          ).overrideWith((ref) => members(++memberBuilds)),
      ],
    );
    addTearDown(container.dispose);
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await session.subscribed;
    await _pumpEventQueue();
    return (container, session);
  }

  List<String> rowIds(ProviderContainer container) => [
    for (final event
        in container.read(channelMessagesProvider(_channelId)).value ??
            const <NostrEvent>[])
      if (event.kind == EventKind.streamMessage) event.id,
  ];

  LocalSendStatus statusOf(ProviderContainer container, String id) {
    final pending = container
        .read(pendingLocalMessagesProvider(_channelId))
        .containsKey(id);
    final failed = container
        .read(failedLocalMessagesProvider(_channelId))
        .contains(id);
    if (!pending) return LocalSendStatus.sent;
    return failed ? LocalSendStatus.failed : LocalSendStatus.pending;
  }

  test(
    'DM send with loaded members shows the row without a member fetch',
    () async {
      final human = 'b' * 64;
      final refetch = Completer<List<ChannelMember>>();
      final (container, session) = await openChannel(
        members: (build) => build == 1
            ? Future.value([_member(self), _member(human)])
            : refetch.future,
      );
      container.listen(channelMembersProvider(_channelId), (_, _) {});
      await container.read(channelMembersProvider(_channelId).future);
      // A membership refresh is in flight and never answers: the send must
      // use the loaded snapshot instead of waiting for it.
      container.invalidate(channelMembersProvider(_channelId));
      expect(
        container.read(channelMembersProvider(_channelId)).isLoading,
        isTrue,
      );

      final send = container
          .read(sendMessageProvider)
          .call(
            channelId: _channelId,
            content: 'hi',
            channel: _dmChannel([self, human]),
            mentionPubkeys: const [],
            keepFailedMessage: true,
          );
      final event = await session.nextPublish;

      expect(refetch.isCompleted, isFalse);
      expect(rowIds(container), [event.id]);
      expect(statusOf(container, event.id), LocalSendStatus.pending);
      expect(event.tags.where((tag) => tag.first == 'p').toList(), [
        ['p', human],
      ]);
      session.accept();
      await send;
      expect(statusOf(container, event.id), LocalSendStatus.sent);
    },
  );

  test(
    'a rejected publish keeps the row, marked failed, and throws nothing',
    () async {
      final (container, session) = await openChannel();
      final send = container
          .read(sendMessageProvider)
          .call(
            channelId: _channelId,
            content: 'hello',
            mentionPubkeys: const [],
            keepFailedMessage: true,
          );
      final event = await session.nextPublish;
      session.reject();
      await send;

      expect(rowIds(container), [event.id]);
      expect(statusOf(container, event.id), LocalSendStatus.failed);
    },
  );

  test(
    'without keepFailedMessage a rejected publish still rolls back',
    () async {
      final (container, session) = await openChannel();
      final send = container
          .read(sendMessageProvider)
          .call(
            channelId: _channelId,
            content: 'cmd',
            mentionPubkeys: const [],
          );
      await session.nextPublish;
      session.reject();

      await expectLater(send, throwsException);
      expect(rowIds(container), isEmpty);
      expect(container.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
    },
  );

  test(
    'retry republishes the same event and its echo leaves one row',
    () async {
      final (container, session) = await openChannel();
      final sendMessage = container.read(sendMessageProvider);
      final send = sendMessage.call(
        channelId: _channelId,
        content: 'again',
        mentionPubkeys: const [],
        keepFailedMessage: true,
      );
      final failed = await session.nextPublish;
      session.reject();
      await send;
      expect(statusOf(container, failed.id), LocalSendStatus.failed);

      final retry = sendMessage.retry(
        channelId: _channelId,
        eventId: failed.id,
      );
      final republished = await session.nextPublish;
      expect(republished.id, failed.id);
      expect(statusOf(container, failed.id), LocalSendStatus.pending);
      session.accept();
      await retry;
      expect(statusOf(container, failed.id), LocalSendStatus.sent);

      session.emit(republished);
      await _pumpEventQueue();
      expect(rowIds(container), [failed.id]);
    },
  );

  test('a failed retry marks the row failed again', () async {
    final (container, session) = await openChannel();
    final sendMessage = container.read(sendMessageProvider);
    final send = sendMessage.call(
      channelId: _channelId,
      content: 'still offline',
      mentionPubkeys: const [],
      keepFailedMessage: true,
    );
    final event = await session.nextPublish;
    session.reject();
    await send;

    final retry = sendMessage.retry(channelId: _channelId, eventId: event.id);
    await session.nextPublish;
    session.reject();
    await retry;
    expect(rowIds(container), [event.id]);
    expect(statusOf(container, event.id), LocalSendStatus.failed);
  });

  test('delete removes the failed row', () async {
    final (container, session) = await openChannel();
    final sendMessage = container.read(sendMessageProvider);
    final send = sendMessage.call(
      channelId: _channelId,
      content: 'never mind',
      mentionPubkeys: const [],
      keepFailedMessage: true,
    );
    final event = await session.nextPublish;
    session.reject();
    await send;

    sendMessage.discard(channelId: _channelId, eventId: event.id);
    expect(rowIds(container), isEmpty);
    expect(container.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
    expect(container.read(failedLocalMessagesProvider(_channelId)), isEmpty);
  });

  test('a failed row survives a reconnect and later live events', () async {
    final (container, session) = await openChannel();
    final send = container
        .read(sendMessageProvider)
        .call(
          channelId: _channelId,
          content: 'lost signal',
          mentionPubkeys: const [],
          keepFailedMessage: true,
        );
    final event = await session.nextPublish;
    session.reject();
    await send;

    session.setConnected(false);
    await _pumpEventQueue();
    session.setConnected(true);
    container.read(channelMessagesProvider(_channelId));
    await _pumpEventQueue();
    await _pumpEventQueue();
    session.emit(_otherMessage(createdAt: event.createdAt + 1));
    await _pumpEventQueue();

    expect(rowIds(container), [event.id, 'other']);
    expect(statusOf(container, event.id), LocalSendStatus.failed);
  });

  testWidgets('own bubble shows a clock while sending, then nothing', (
    tester,
  ) async {
    final (container, session) =
        await tester.runAsync(openChannel) ?? (null, null);
    await tester.pumpWidget(_StatusHarness(container: container!));

    final send = container
        .read(sendMessageProvider)
        .call(
          channelId: _channelId,
          content: 'tick',
          mentionPubkeys: const [],
          keepFailedMessage: true,
        );
    final event = await session!.nextPublish;
    await tester.pump();
    expect(find.byKey(ValueKey('local-send-pending-${event.id}')), findsOne);
    final semantics = tester.ensureSemantics();
    await tester.pump();
    expect(find.bySemanticsLabel('Sending'), findsOne);
    semantics.dispose();

    session.accept();
    await send;
    await tester.pump();
    expect(find.byIcon(Icons.schedule), findsNothing);
    expect(find.text('Not sent'), findsNothing);
  });

  testWidgets('tapping a failed mark offers retry and delete', (tester) async {
    final (container, session) =
        await tester.runAsync(openChannel) ?? (null, null);
    await tester.pumpWidget(_StatusHarness(container: container!));

    final send = container
        .read(sendMessageProvider)
        .call(
          channelId: _channelId,
          content: 'tock',
          mentionPubkeys: const [],
          keepFailedMessage: true,
        );
    final event = await session!.nextPublish;
    session.reject();
    await send;
    await tester.pump();
    expect(find.text('Not sent'), findsOne);
    expect(find.byIcon(Icons.error_outline), findsOne);

    await tester.tap(find.byKey(ValueKey('local-send-failed-${event.id}')));
    await tester.pumpAndSettle();
    expect(find.text('Retry'), findsOne);
    await tester.tap(find.text('Retry'));
    await tester.pumpAndSettle();
    final republished = await session.nextPublish;
    expect(republished.id, event.id);
    await tester.pump();
    expect(find.text('Not sent'), findsNothing);
    expect(find.byIcon(Icons.schedule), findsOne);
    session.reject();
    await tester.pumpAndSettle();
    expect(find.text('Not sent'), findsOne);

    await tester.tap(find.byKey(ValueKey('local-send-failed-${event.id}')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(find.byType(ChatBubbleRow), findsNothing);
    expect(container.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
  });
}

/// Mirrors how the channel and thread bubbles feed [ChatBubbleRow.status].
class _StatusHarness extends StatelessWidget {
  final ProviderContainer container;

  const _StatusHarness({required this.container});

  @override
  Widget build(BuildContext context) {
    return UncontrolledProviderScope(
      container: container,
      child: MaterialApp(
        home: Scaffold(
          body: Consumer(
            builder: (context, ref, _) {
              final rows =
                  ref.watch(channelMessagesProvider(_channelId)).value ??
                  const <NostrEvent>[];
              return ListView(
                children: [
                  for (final event in rows)
                    Builder(
                      builder: (context) {
                        final status = watchLocalSendStatus(
                          ref,
                          _channelId,
                          event.id,
                        );
                        return ChatBubbleRow(
                          isOwn: true,
                          leading: const SizedBox.shrink(),
                          showTime: true,
                          createdAt: event.createdAt,
                          status: status == LocalSendStatus.sent
                              ? null
                              : LocalSendStatusMark(
                                  status: status,
                                  channelId: _channelId,
                                  eventId: event.id,
                                ),
                          child: Text(event.content),
                        );
                      },
                    ),
                ],
              );
            },
          ),
        ),
      ),
    );
  }
}

class _FixedConfig extends RelayConfigNotifier {
  final String nsec;

  _FixedConfig(this.nsec);

  @override
  RelayConfig build() =>
      RelayConfig(baseUrl: 'https://relay.example', nsec: nsec);
}

/// A connected session whose channel window is empty and whose publishes the
/// test answers one at a time.
class _ControlledSession extends RelaySessionNotifier {
  final _listeners = <void Function(NostrEvent)>[];
  final _subscribed = Completer<void>();
  final _announced = <Completer<NostrEvent>>[];
  var _publishCount = 0;
  var _readCount = 0;
  Completer<NostrEvent>? _current;
  NostrEvent? _currentEvent;

  Future<void> get subscribed => _subscribed.future;

  Completer<NostrEvent> _slot(int index) {
    while (_announced.length <= index) {
      _announced.add(Completer<NostrEvent>());
    }
    return _announced[index];
  }

  /// The next event handed to [publish] that the test hasn't read yet.
  Future<NostrEvent> get nextPublish => _slot(_readCount++).future;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  void setConnected(bool connected) {
    state = SessionState(
      status: connected ? SessionStatus.connected : SessionStatus.disconnected,
    );
  }

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async => [_bounds()];

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async => const [];

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    if (!_subscribed.isCompleted) _subscribed.complete();
    _listeners.add(onEvent);
    return () => _listeners.remove(onEvent);
  }

  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) {
    final result = Completer<NostrEvent>();
    _current = result;
    _currentEvent = event;
    _slot(_publishCount++).complete(event);
    return result.future;
  }

  void accept() => _current!.complete(_currentEvent!);

  void reject() => _current!.completeError(Exception('relay rejected event'));

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}

ChannelMember _member(String pubkey) =>
    ChannelMember(pubkey: pubkey, role: 'member', joinedAt: DateTime(2025));

Channel _dmChannel(List<String> participantPubkeys) => Channel(
  id: _channelId,
  name: 'DM',
  channelType: 'dm',
  visibility: 'private',
  description: '',
  createdBy: participantPubkeys.first,
  createdAt: DateTime(2025),
  memberCount: participantPubkeys.length,
  participantPubkeys: participantPubkeys,
  isMember: true,
);

NostrEvent _otherMessage({required int createdAt}) => NostrEvent(
  id: 'other',
  pubkey: 'c' * 64,
  createdAt: createdAt,
  kind: EventKind.streamMessage,
  tags: const [
    ['h', _channelId],
  ],
  content: 'from someone else',
  sig: 'sig',
);

NostrEvent _bounds() => NostrEvent(
  id: 'bounds',
  pubkey: 'relay',
  createdAt: 0,
  kind: EventKind.channelWindowBounds,
  tags: [
    ['d', '${_channelId.toLowerCase()}:head'],
  ],
  content: jsonEncode({'has_more': false, 'next_cursor': null}),
  sig: 'sig',
);

Future<void> _pumpEventQueue() async {
  await Future<void>.delayed(Duration.zero);
  await Future<void>.delayed(Duration.zero);
}
