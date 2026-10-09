import 'dart:convert';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:buzz/features/channels/agent_activity/observer_watching.dart';
import 'package:buzz/shared/crypto/nip44.dart';
import 'package:buzz/shared/relay/relay.dart';

void main() {
  final owner = nostr.Keys.generate();
  final agent = nostr.Keys.generate();

  test('the watching frame is an owner-signed control frame to the agent', () {
    final event = buildWatchingControlEvent(
      nsec: owner.nsec,
      agentPubkey: agent.public,
      channelId: 'chan-1',
    );
    expect(event.kind, EventKind.agentObserverFrame);
    expect(event.pubkey, owner.public);
    expect(event.tags, [
      ['p', agent.public],
      ['agent', agent.public],
      ['frame', 'control'],
    ]);
    final verified = nostr.Event(
      event.id,
      event.pubkey,
      event.createdAt,
      event.kind,
      event.tags,
      event.content,
      event.sig!,
      verify: false,
    );
    expect(verified.isValid(), isTrue, reason: 'signed by the owner');

    // The agent decrypts with its own key and the owner's pubkey.
    final plaintext = nip44Decrypt(
      getConversationKey(agent.secret, owner.public),
      event.content,
    );
    expect(jsonDecode(plaintext), {'type': 'watching', 'channelId': 'chan-1'});
  });

  group('useObserverWatching', () {
    late _RecordingSession session;
    late _Lifecycle lifecycle;

    Future<void> pumpSheet(WidgetTester tester, {bool open = true}) async {
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            relaySessionProvider.overrideWith(() => session),
            relayConfigProvider.overrideWith(() => _Config(owner.nsec)),
            appLifecycleProvider.overrideWith(() => lifecycle),
          ],
          child: open ? _Watcher(agentPubkey: agent.public) : const _Closed(),
        ),
      );
      await tester.pump();
    }

    setUp(() {
      session = _RecordingSession();
      lifecycle = _Lifecycle();
    });

    testWidgets('sends on open and every 30 s while open', (tester) async {
      await pumpSheet(tester);
      expect(session.published, hasLength(1));
      await tester.pump(const Duration(seconds: 29));
      expect(session.published, hasLength(1));
      await tester.pump(const Duration(seconds: 1));
      expect(session.published, hasLength(2));
      await tester.pump(const Duration(seconds: 60));
      expect(session.published, hasLength(4));
      final plaintext = nip44Decrypt(
        getConversationKey(agent.secret, owner.public),
        session.published.last.content,
      );
      expect(jsonDecode(plaintext)['type'], 'watching');
    });

    testWidgets('stops when the sheet closes', (tester) async {
      await pumpSheet(tester);
      await pumpSheet(tester, open: false);
      await tester.pump(const Duration(minutes: 2));
      expect(session.published, hasLength(1));
    });

    testWidgets('stops in the background and resumes at once', (tester) async {
      await pumpSheet(tester);
      lifecycle.set(AppLifecycleState.paused);
      await tester.pump();
      await tester.pump(const Duration(minutes: 2));
      expect(session.published, hasLength(1));

      lifecycle.set(AppLifecycleState.resumed);
      await tester.pump();
      expect(session.published, hasLength(2));
      await tester.pump(const Duration(seconds: 30));
      expect(session.published, hasLength(3));
    });

    testWidgets('pauses while disconnected and sends at once on reconnect', (
      tester,
    ) async {
      await pumpSheet(tester);
      session.setStatus(SessionStatus.reconnecting);
      await tester.pump();
      await tester.pump(const Duration(minutes: 2));
      expect(session.published, hasLength(1));

      session.setStatus(SessionStatus.connected);
      await tester.pump();
      expect(session.published, hasLength(2), reason: 'immediate on reconnect');
      await tester.pump(const Duration(seconds: 30));
      expect(session.published, hasLength(3));
    });
  });
}

class _Watcher extends HookConsumerWidget {
  const _Watcher({required this.agentPubkey});

  final String agentPubkey;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    useObserverWatching(ref, channelId: 'chan-1', agentPubkey: agentPubkey);
    return const SizedBox();
  }
}

class _Closed extends StatelessWidget {
  const _Closed();

  @override
  Widget build(BuildContext context) => const SizedBox();
}

class _RecordingSession extends RelaySessionNotifier {
  final List<NostrEvent> published = [];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  void setStatus(SessionStatus status) {
    state = SessionState(status: status);
  }

  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    published.add(event);
    return event;
  }
}

class _Lifecycle extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;

  void set(AppLifecycleState next) => state = next;
}

class _Config extends RelayConfigNotifier {
  _Config(this._nsec);

  final String _nsec;

  @override
  RelayConfig build() =>
      RelayConfig(baseUrl: 'http://localhost:3000', nsec: _nsec);
}
