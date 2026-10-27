import 'dart:async';
import 'dart:convert';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/shared/auth/auth_provider.dart';
import 'package:kuzz/features/age_gate/age_signal_provider.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_capture.dart';
import 'package:kuzz/shared/relay/relay.dart';

class _Auth extends AuthNotifier {
  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.authenticated);
}

class _Age extends AgeSignalNotifier {
  @override
  AgeSignalState build() => AgeSignalState.allowed;
}

class _Config extends RelayConfigNotifier {
  _Config(this.nsec);
  final String nsec;
  @override
  RelayConfig build() => RelayConfig(baseUrl: 'https://one', nsec: nsec);
}

class _Socket extends RelaySocket {
  _Socket({
    required super.wsUrl,
    required super.nsec,
    required super.onMessage,
    required super.onConnected,
    required super.onDisconnected,
  });
  final frames = <List<dynamic>>[];
  @override
  Future<void> connect() async {}
  @override
  void send(List<dynamic> frame) {
    frames.add(frame);
  }

  @override
  void dispose() {}
}

void main() {
  test(
    'real session persists HTTP and live replies while delivering normally; logout fences late work',
    () async {
      final keys = nostr.Keys.generate();
      final store = MessageStore(
        () async => inMemoryDatabasePath,
        factory: databaseFactoryFfiNoIsolate,
      );
      final event = NostrEvent(
        id: 'reply',
        pubkey: keys.public,
        createdAt: 1,
        kind: 9,
        tags: [
          ['h', 'room'],
          ['e', 'root'],
        ],
        content: 'hello',
      );
      final response = Completer<http.Response>();
      late _Socket socket;
      final session = RelaySessionNotifier(
        httpClient: MockClient((_) => response.future),
        socketFactory:
            ({
              required wsUrl,
              required nsec,
              required onMessage,
              required onConnected,
              required onDisconnected,
            }) {
              return socket = _Socket(
                wsUrl: wsUrl,
                nsec: nsec,
                onMessage: onMessage,
                onConnected: onConnected,
                onDisconnected: onDisconnected,
              );
            },
      );
      final container = ProviderContainer(
        overrides: [
          authProvider.overrideWith(_Auth.new),
          ageSignalProvider.overrideWith(_Age.new),
          relayConfigProvider.overrideWith(() => _Config(keys.nsec)),
          messageStoreProvider.overrideWithValue(store),
          relaySessionProvider.overrideWith(() => session),
        ],
      );
      await container.read(authProvider.future);
      container.read(relaySessionProvider);
      await Future<void>.delayed(Duration.zero);
      session.debugAttachSocketForTest(socket);
      await session.debugHandleConnected();
      final received = <NostrEvent>[];
      final stop = session.subscribe(
        const NostrFilter(
          kinds: [9],
          tags: {
            '#h': ['room'],
          },
          limit: 0,
        ),
        received.add,
      );
      await Future<void>.delayed(Duration.zero);
      final req = socket.frames.firstWhere((f) => f[0] == 'REQ');
      session.debugHandleMessage(['EVENT', req[1], event.toJson()]);
      session.debugFlushEventBuffer();
      expect(received.map((e) => e.id), ['reply']);
      await Future<void>.delayed(Duration.zero);
      var reader = store.begin('https://one', keys.public);
      expect((await store.get(reader, 'reply'))?.content, 'hello');

      final query = session.queryRelay(const [
        NostrFilter(kinds: [9]),
      ]);
      response.complete(
        http.Response(jsonEncode([event.toJson()..['id'] = 'history']), 200),
      );
      expect((await query).single.id, 'history');
      await Future<void>.delayed(Duration.zero);
      expect(await store.get(reader, 'history'), isNotNull);

      await store.clear('https://one', keys.public);
      session.debugHandleMessage([
        'EVENT',
        req[1],
        event.toJson()..['id'] = 'late',
      ]);
      session.debugFlushEventBuffer();
      await Future<void>.delayed(Duration.zero);
      reader = store.begin('https://one', keys.public);
      expect(await store.get(reader, 'late'), null);
      session.debugHandleMessage(['EOSE', req[1]]);
      (await stop)();
      container.dispose();
      await store.close();
    },
  );
}
