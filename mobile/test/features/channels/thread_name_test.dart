import 'dart:async';

import 'package:buzz/features/channels/thread_name_editor.dart';
import 'package:buzz/features/channels/thread_name_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme_provider.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

const target = (channelId: 'channel', headId: 'head');
const principal =
    'abababababababababababababababababababababababababababababababab';

NostrEvent named(
  String name,
  int time, {
  String id = 'a',
  String channel = 'channel',
}) => NostrEvent(
  id: id,
  pubkey: principal,
  createdAt: time,
  kind: EventKind.threadName,
  tags: [
    ['h', channel],
    ['e', 'head'],
  ],
  content: name,
);

class Config extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://relay.test',
    tokenAuth: true,
    principalId: principal,
  );
}

class Session extends RelaySessionNotifier {
  List<NostrEvent> history = [];
  void Function(NostrEvent)? live;
  bool stopped = false;
  bool reject = false;
  NostrEvent? published;
  Completer<List<NostrEvent>>? historyGate;
  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);
  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    live = onEvent;
    return () {
      stopped = true;
    };
  }

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async => historyGate?.future ?? history;
  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    if (reject) throw StateError('Save rejected');
    published = event;
    final stored = named(
      event.content,
      event.createdAt,
      id: 'relay-${event.createdAt}',
    );
    history = [stored];
    live?.call(stored);
    return stored;
  }
}

void main() {
  late SharedPreferences prefs;
  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    prefs = await SharedPreferences.getInstance();
  });

  test('language limits and latest clear share the desktop contract', () {
    expect(threadNameError('a' * 40), isNull);
    expect(threadNameError('가' * 20), isNull);
    expect(threadNameError('가' * 10 + 'a' * 20), isNull);
    expect(threadNameError('가' * 21), isNotNull);
    expect(threadNameError('line\nbreak'), isNotNull);
    expect(
      latestThreadName(target, [named('old', 1), named('', 2)])?.content,
      '',
    );
    expect(
      latestThreadName(target, [
        named('z', 2, id: 'z'),
        named('a', 2),
        named('other channel', 9, channel: 'other'),
      ])?.content,
      'a',
    );
  });

  test(
    'cache and drafts survive reopening but cannot cross identities',
    () async {
      final store = ThreadNameStorage(prefs, 'https://one.test:alice');
      await store.write(target, named('이름', 2));
      await store.saveDraft(target, '수정 중');
      await store.write(target, named('stale', 1));
      expect(ThreadNameStorage(prefs, store.scope).read(target)?.content, '이름');
      expect(ThreadNameStorage(prefs, store.scope).draft(target), '수정 중');
      expect(
        ThreadNameStorage(prefs, 'https://two.test:alice').read(target),
        isNull,
      );
      expect(
        ThreadNameStorage(prefs, 'https://one.test:bob').draft(target),
        isNull,
      );
    },
  );

  test(
    'live update beats delayed history and dispose closes subscription',
    () async {
      final gate = Completer<List<NostrEvent>>();
      final session = Session()..historyGate = gate;
      final container = ProviderContainer(
        overrides: [
          savedPrefsProvider.overrideWithValue(prefs),
          relayConfigProvider.overrideWith(Config.new),
          relaySessionProvider.overrideWith(() => session),
        ],
      );
      final sub = container.listen(threadNameProvider(target), (_, _) {});
      await Future<void>.delayed(Duration.zero);
      session.live!(named('live', 3));
      gate.complete([named('stale', 2)]);
      await Future<void>.delayed(Duration.zero);
      expect(container.read(threadNameProvider(target)).value?.content, 'live');
      sub.close();
      container.dispose();
      expect(session.stopped, isTrue);
      session.live!(named('late', 4));
    },
  );

  testWidgets('editor enforces limits, keeps failed draft, and saves/clears', (
    tester,
  ) async {
    final session = Session()..history = [named('기존 이름', 2000000000)];
    final container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        relayConfigProvider.overrideWith(Config.new),
        relaySessionProvider.overrideWith(() => session),
      ],
    );
    addTearDown(container.dispose);
    final sub = container.listen(threadNameProvider(target), (_, _) {});
    addTearDown(sub.close);
    await tester.pump();
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () => showThreadNameEditor(context, target),
                child: const Text('Edit'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Edit'));
    await tester.pumpAndSettle();
    expect(find.text('기존 이름'), findsOneWidget);
    await tester.enterText(
      find.byKey(const ValueKey('thread-name-input')),
      '가' * 21,
    );
    await tester.pump();
    expect(
      tester
          .widget<FilledButton>(find.byKey(const ValueKey('thread-name-save')))
          .onPressed,
      isNull,
    );
    await tester.enterText(
      find.byKey(const ValueKey('thread-name-input')),
      '새 이름',
    );
    await tester.pump();
    session.reject = true;
    await tester.tap(find.byKey(const ValueKey('thread-name-save')));
    await tester.pumpAndSettle();
    expect(find.textContaining('Save rejected'), findsOneWidget);
    expect(container.read(threadNameStorageProvider).draft(target), '새 이름');
    session.reject = false;
    await tester.tap(find.byKey(const ValueKey('thread-name-save')));
    await tester.pumpAndSettle();
    expect(session.published?.createdAt, 2000000001);
    expect(
      container.read(threadNameProvider(target)).value?.id,
      'relay-2000000001',
    );
    expect(container.read(threadNameStorageProvider).draft(target), isNull);
    await tester.tap(find.text('Edit'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const ValueKey('thread-name-input')), '');
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('thread-name-save')));
    await tester.pumpAndSettle();
    expect(session.published?.content, '');
    expect(session.published?.createdAt, 2000000002);
  });
}
