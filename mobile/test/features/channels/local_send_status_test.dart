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
import 'package:kuzz/features/channels/local_send_store.dart';
import 'package:kuzz/features/channels/pending_local_messages_provider.dart';
import 'package:kuzz/features/channels/send_message_provider.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _channelId = '11111111-1111-4111-8111-111111111111';

void main() {
  late String nsec;
  late String self;

  setUp(() {
    final keys = nostr.Keys.generate();
    nsec = keys.nsec;
    self = keys.public;
  });

  late _FixedConfig config;

  Future<(ProviderContainer, _ControlledSession)> openChannel({
    Future<List<ChannelMember>> Function(int build)? members,
    SharedPreferences? prefs,
    String? signer,
    Future<List<NostrEvent>> Function(NostrFilter filter)? lookup,
  }) async {
    final session = _ControlledSession()..lookup = lookup;
    var memberBuilds = 0;
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        relayConfigProvider.overrideWith(
          () => config = _FixedConfig(signer ?? nsec),
        ),
        if (prefs != null) savedPrefsProvider.overrideWithValue(prefs),
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

  test('failed sends stay with the account that wrote them', () async {
    final (container, session) = await openChannel();
    final send = container
        .read(sendMessageProvider)
        .call(
          channelId: _channelId,
          content: 'mine',
          mentionPubkeys: const [],
          keepFailedMessage: true,
        );
    final event = await session.nextPublish;
    session.reject();
    await send;
    expect(rowIds(container), [event.id]);

    config.switchTo(nostr.Keys.generate().nsec);
    await _pumpEventQueue();
    expect(rowIds(container), isEmpty);
    await expectLater(
      container
          .read(sendMessageProvider)
          .retry(channelId: _channelId, eventId: event.id),
      throwsA(isA<StateError>()),
    );

    config.switchTo(nsec);
    await _pumpEventQueue();
    expect(rowIds(container), [event.id]);
    expect(statusOf(container, event.id), LocalSendStatus.failed);
  });

  test('unconfirmed thread replies show only for their author', () async {
    const args = ThreadRepliesArgs(channelId: _channelId, rootId: 'root');
    final config = _FixedConfig(nsec);
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(_ControlledSession.new),
        relayConfigProvider.overrideWith(() => config),
        threadRepliesProvider(args).overrideWith((ref) async => const []),
      ],
    );
    addTearDown(container.dispose);
    container.listen(threadRepliesWithLocalProvider(args), (_, _) {});
    final reply = _reply(self);
    container.read(threadLocalRepliesProvider(args).notifier).add(reply);
    await _pumpEventQueue();
    List<String> shown() => [
      for (final event
          in container.read(threadRepliesWithLocalProvider(args)).value ??
              const <NostrEvent>[])
        event.id,
    ];
    expect(shown(), [reply.id]);

    config.switchTo(nostr.Keys.generate().nsec);
    await _pumpEventQueue();
    expect(shown(), isEmpty);
  });

  group('stored unsent messages', () {
    /// Ids of the local sends kept on the device, by signer.
    Map<String, List<String>> stored(SharedPreferences prefs) => {
      for (final key in prefs.getKeys())
        if (key.startsWith('local_sends_v1:'))
          key.split(':').reversed.elementAt(1): [
            for (final entry
                in jsonDecode(prefs.getString(key)!) as List<dynamic>)
              (entry as Map<String, dynamic>)['id'] as String,
          ],
    };

    /// Closes [container] and reopens the preferences as a new app run does.
    Future<SharedPreferences> restart(
      ProviderContainer container,
      SharedPreferences prefs,
    ) async {
      container.dispose();
      final values = {
        for (final key in prefs.getKeys()) key: prefs.getString(key) as Object,
      };
      SharedPreferences.resetStatic();
      SharedPreferences.setMockInitialValues(values);
      return SharedPreferences.getInstance();
    }

    /// Sends one message that fails, then restarts the app. [lookup] answers
    /// the restart's relay lookup by id.
    Future<(ProviderContainer, _ControlledSession, NostrEvent)>
    failThenRestart({
      Future<List<NostrEvent>> Function(NostrFilter filter)? lookup,
      String? parentEventId,
    }) async {
      SharedPreferences.setMockInitialValues({});
      var prefs = await SharedPreferences.getInstance();
      final (first, session) = await openChannel(prefs: prefs);
      final send = first
          .read(sendMessageProvider)
          .call(
            channelId: _channelId,
            content: 'offline',
            parentEventId: parentEventId,
            mentionPubkeys: const [],
            keepFailedMessage: true,
          );
      final event = await session.nextPublish;
      session.reject();
      await send;
      expect(stored(prefs), {
        self: [event.id],
      });

      prefs = await restart(first, prefs);
      final (container, next) = await openChannel(prefs: prefs, lookup: lookup);
      await _pumpEventQueue();
      return (container, next, event);
    }

    tearDown(SharedPreferences.resetStatic);

    test('a failed row comes back after a restart, still failed', () async {
      final (container, _, event) = await failThenRestart();
      expect(rowIds(container), [event.id]);
      expect(statusOf(container, event.id), LocalSendStatus.failed);
    });

    test('retrying a restored row resends it and clears the store', () async {
      final (container, session, event) = await failThenRestart();
      final prefs = container.read(savedPrefsProvider);
      final retry = container
          .read(sendMessageProvider)
          .retry(channelId: _channelId, eventId: event.id);
      final republished = await session.nextPublish;
      expect(republished.id, event.id);
      session.accept();
      await retry;

      expect(rowIds(container), [event.id]);
      expect(statusOf(container, event.id), LocalSendStatus.sent);
      expect(stored(prefs), isEmpty);
    });

    test('deleting a restored row clears the store', () async {
      final (container, _, event) = await failThenRestart();
      final prefs = container.read(savedPrefsProvider);
      container
          .read(sendMessageProvider)
          .discard(channelId: _channelId, eventId: event.id);

      expect(rowIds(container), isEmpty);
      expect(container.read(failedLocalMessagesProvider(_channelId)), isEmpty);
      expect(stored(prefs), isEmpty);
    });

    test('the echo of a restored row replaces it once', () async {
      final (container, session, event) = await failThenRestart();
      final prefs = container.read(savedPrefsProvider);
      session.emit(event);
      await _pumpEventQueue();

      expect(rowIds(container), [event.id]);
      expect(statusOf(container, event.id), LocalSendStatus.sent);
      expect(stored(prefs), isEmpty);
    });

    test('another account never sees the stored row', () async {
      final (container, _, event) = await failThenRestart();
      final prefs = await restart(
        container,
        container.read(savedPrefsProvider),
      );
      final (other, _) = await openChannel(
        prefs: prefs,
        signer: nostr.Keys.generate().nsec,
      );
      await _pumpEventQueue();

      expect(rowIds(other), isEmpty);
      expect(other.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
      expect(stored(prefs), {
        self: [event.id],
      });
    });

    test('a send cut off by the app closing comes back failed', () async {
      SharedPreferences.setMockInitialValues({});
      var prefs = await SharedPreferences.getInstance();
      final (first, session) = await openChannel(prefs: prefs);
      unawaited(
        first
            .read(sendMessageProvider)
            .call(
              channelId: _channelId,
              content: 'in flight',
              mentionPubkeys: const [],
              keepFailedMessage: true,
            ),
      );
      final event = await session.nextPublish;
      expect(statusOf(first, event.id), LocalSendStatus.pending);

      prefs = await restart(first, prefs);
      final (container, _) = await openChannel(prefs: prefs);
      await _pumpEventQueue();
      expect(rowIds(container), [event.id]);
      expect(statusOf(container, event.id), LocalSendStatus.failed);
    });

    test('a restored row the relay has is confirmed and forgotten', () async {
      NostrEvent? delivered;
      final lookups = <NostrFilter>[];
      final (container, _, event) = await failThenRestart(
        lookup: (filter) async {
          lookups.add(filter);
          return [?delivered];
        },
      );
      expect(statusOf(container, event.id), LocalSendStatus.failed);

      // Restart once more, now with the relay holding the event.
      delivered = event;
      final prefs = await restart(
        container,
        container.read(savedPrefsProvider),
      );
      final (reopened, _) = await openChannel(
        prefs: prefs,
        lookup: (filter) async {
          lookups.add(filter);
          return [?delivered];
        },
      );
      await _pumpEventQueue();

      expect(lookups.last.ids, [event.id]);
      expect(lookups.last.tags['#h'], [_channelId]);
      expect(rowIds(reopened), [event.id]);
      expect(statusOf(reopened, event.id), LocalSendStatus.sent);
      expect(stored(prefs), isEmpty);
    });

    test(
      'a restored row waits for the lookup; a failed one marks it',
      () async {
        final answer = Completer<List<NostrEvent>>();
        SharedPreferences.setMockInitialValues({});
        var prefs = await SharedPreferences.getInstance();
        final (first, session) = await openChannel(prefs: prefs);
        final send = first
            .read(sendMessageProvider)
            .call(
              channelId: _channelId,
              content: 'unknown',
              mentionPubkeys: const [],
              keepFailedMessage: true,
            );
        final event = await session.nextPublish;
        session.reject();
        await send;

        prefs = await restart(first, prefs);
        final (container, _) = await openChannel(
          prefs: prefs,
          lookup: (_) => answer.future,
        );
        await _pumpEventQueue();
        expect(rowIds(container), [event.id]);
        expect(statusOf(container, event.id), LocalSendStatus.pending);

        answer.completeError(Exception('offline'));
        await _pumpEventQueue();
        expect(statusOf(container, event.id), LocalSendStatus.failed);
        expect(stored(prefs), {
          self: [event.id],
        });
      },
    );

    test('retrying an unverified row the relay has sends nothing', () async {
      final (container, session, event) = await failThenRestart(
        lookup: (_) async => throw Exception('offline'),
      );
      expect(statusOf(container, event.id), LocalSendStatus.failed);
      final prefs = container.read(savedPrefsProvider);

      session.lookup = (filter) async => [
        if (filter.ids!.contains(event.id)) event,
      ];
      await container
          .read(sendMessageProvider)
          .retry(channelId: _channelId, eventId: event.id);

      expect(session._publishCount, 0);
      expect(rowIds(container), [event.id]);
      expect(statusOf(container, event.id), LocalSendStatus.sent);
      expect(stored(prefs), isEmpty);
    });

    for (final relayAnswers in [true, false]) {
      test('retrying an unverified row sends it when the relay '
          '${relayAnswers ? 'lacks it' : 'cannot say'}', () async {
        final (container, session, event) = await failThenRestart(
          lookup: (_) async => throw Exception('offline'),
        );
        final prefs = container.read(savedPrefsProvider);
        var lookups = 0;
        session.lookup = (_) async {
          lookups++;
          if (relayAnswers) return const [];
          throw Exception('still offline');
        };
        final retry = container
            .read(sendMessageProvider)
            .retry(channelId: _channelId, eventId: event.id);
        final republished = await session.nextPublish;
        expect(lookups, 1);
        expect(republished.id, event.id);
        session.accept();
        await retry;

        expect(statusOf(container, event.id), LocalSendStatus.sent);
        expect(stored(prefs), isEmpty);
      });
    }

    test('a verified row is retried without another lookup', () async {
      var lookups = 0;
      final (container, session, event) = await failThenRestart(
        lookup: (_) async {
          lookups++;
          return const [];
        },
      );
      expect(lookups, 1);
      final retry = container
          .read(sendMessageProvider)
          .retry(channelId: _channelId, eventId: event.id);
      await session.nextPublish;
      session.accept();
      await retry;
      expect(lookups, 1);
    });

    test('a send without a kept failed row is never stored', () async {
      SharedPreferences.setMockInitialValues({});
      var prefs = await SharedPreferences.getInstance();
      final (first, session) = await openChannel(prefs: prefs);
      // Agent commands and thread-close markers send this way.
      unawaited(
        first
            .read(sendMessageProvider)
            .call(
              channelId: _channelId,
              content: '/agent do it',
              mentionPubkeys: const [],
            )
            .catchError((Object _) {}),
      );
      final event = await session.nextPublish;
      expect(statusOf(first, event.id), LocalSendStatus.pending);
      expect(stored(prefs), isEmpty);

      prefs = await restart(first, prefs);
      final (container, _) = await openChannel(prefs: prefs);
      await _pumpEventQueue();
      expect(rowIds(container), isEmpty);
      expect(container.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
    });

    test('a failed thread reply comes back in its thread', () async {
      const args = ThreadRepliesArgs(channelId: _channelId, rootId: 'root');
      final (container, _, event) = await failThenRestart(
        parentEventId: 'root',
      );
      expect(
        container.read(threadLocalRepliesProvider(args)).map((e) => e.id),
        [event.id],
      );
      expect(rowIds(container), isEmpty);
      expect(statusOf(container, event.id), LocalSendStatus.failed);

      container
          .read(sendMessageProvider)
          .discard(channelId: _channelId, eventId: event.id);
      expect(container.read(threadLocalRepliesProvider(args)), isEmpty);
      expect(stored(container.read(savedPrefsProvider)), isEmpty);
    });

    test('the store keeps the newest 50 sends of the last 7 days', () async {
      final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
      NostrEvent sent(String id, int createdAt) => NostrEvent(
        id: id,
        pubkey: self,
        createdAt: createdAt,
        kind: EventKind.streamMessage,
        tags: const [
          ['h', _channelId],
        ],
        content: id,
        sig: 'sig',
      );
      final expired = sent('expired', now - const Duration(days: 8).inSeconds);
      SharedPreferences.setMockInitialValues({});
      final prefs = await SharedPreferences.getInstance();
      final store = LocalSendStore(prefs, 'https://relay.example');
      final key = store.keyFor(self, _channelId);
      await prefs.setString(key, jsonEncode([expired.toJson()]));
      expect(store.read(key), isEmpty);
      expect(prefs.containsKey(key), isFalse);

      for (var i = 0; i < 55; i++) {
        store.put(key, sent('m$i', now - 100 + i));
      }
      store.put(key, expired);
      expect(store.read(key).map((e) => e.id), [
        for (var i = 5; i < 55; i++) 'm$i',
      ]);
    });

    testWidgets('a restored row shows Not sent', (tester) async {
      final (container, _, event) =
          await tester.runAsync(failThenRestart) ?? (null, null, null);
      await tester.pumpWidget(_StatusHarness(container: container!));
      expect(find.text('offline'), findsOne);
      expect(find.byKey(ValueKey('local-send-failed-${event!.id}')), findsOne);
      expect(find.text('Not sent'), findsOne);
    });
  });

  test('a retry that cannot start names the reason', () {
    expect(
      retryFailureMessage(StateError('Message was written by another account')),
      'Message not sent: it was written by another account',
    );
    expect(
      retryFailureMessage(
        Exception('Cannot submit event: no signing key available'),
      ),
      'Message not sent: no signing key for this account',
    );
    expect(
      retryFailureMessage(
        StateError(
          'Message delivery cancelled because the active community changed',
        ),
      ),
      'Message not sent: the community changed',
    );
  });

  testWidgets('the bubble keeps its size and place when the clock clears', (
    tester,
  ) async {
    final (container, session) =
        await tester.runAsync(openChannel) ?? (null, null);
    await tester.pumpWidget(_StatusHarness(container: container!));
    final long = List.filled(40, 'word').join(' ');

    final send = container
        .read(sendMessageProvider)
        .call(
          channelId: _channelId,
          content: long,
          mentionPubkeys: const [],
          keepFailedMessage: true,
        );
    await session!.nextPublish;
    await tester.pump();
    expect(find.byIcon(Icons.schedule), findsOne);
    final pending = tester.getRect(find.text(long));
    final pendingTime = tester.getRect(find.byKey(const ValueKey('time')));

    session.accept();
    await send;
    await tester.pump();
    expect(find.byIcon(Icons.schedule), findsNothing);
    expect(tester.getRect(find.text(long)), pending);
    expect(tester.getRect(find.byKey(const ValueKey('time'))), pendingTime);
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

    await tester.tapAt(
      tester.getCenter(find.byKey(ValueKey('local-send-failed-${event.id}'))),
    );
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

    await tester.tapAt(
      tester.getCenter(find.byKey(ValueKey('local-send-failed-${event.id}'))),
    );
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
                        // The row, not the overhanging mark, takes the tap.
                        return GestureDetector(
                          behavior: HitTestBehavior.opaque,
                          onTap: status == LocalSendStatus.failed
                              ? () => showFailedLocalMessageSheet(
                                  context: context,
                                  ref: ref,
                                  channelId: _channelId,
                                  eventId: event.id,
                                )
                              : null,
                          child: ChatBubbleRow(
                            isOwn: true,
                            leading: const SizedBox.shrink(),
                            showTime: true,
                            createdAt: event.createdAt,
                            timestampKey: const ValueKey('time'),
                            status: status == LocalSendStatus.sent
                                ? null
                                : LocalSendStatusMark(
                                    status: status,
                                    channelId: _channelId,
                                    eventId: event.id,
                                  ),
                            child: Text(event.content),
                          ),
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

  /// Signs in as another account (or back), as the account switcher does.
  void switchTo(String nsec) =>
      state = RelayConfig(baseUrl: 'https://relay.example', nsec: nsec);
}

/// A connected session whose channel window is empty and whose publishes the
/// test answers one at a time.
class _ControlledSession extends RelaySessionNotifier {
  /// Answers a lookup by event id; none finds nothing.
  Future<List<NostrEvent>> Function(NostrFilter filter)? lookup;
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
  }) async {
    final filter = filters.single;
    if (filter.ids != null) return await lookup?.call(filter) ?? const [];
    return [_bounds()];
  }

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

NostrEvent _reply(String author) => NostrEvent(
  id: 'reply',
  pubkey: author,
  createdAt: 1,
  kind: EventKind.streamMessage,
  tags: const [
    ['h', _channelId],
    ['e', 'root', '', 'root'],
  ],
  content: 'reply',
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
