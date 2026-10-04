import 'dart:convert';

import 'package:buzz/shared/auth/auth_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import 'fake_relay_access_tokens.dart';

const _principal =
    'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22';

const _tokenConfig = RelayConfig(
  baseUrl: 'https://relay.example',
  tokenAuth: true,
  principalId: _principal,
);

class _SendingSocket extends RelaySocket {
  _SendingSocket({
    required super.wsUrl,
    required super.nsec,
    required super.onMessage,
    required super.onConnected,
    required super.onDisconnected,
  }) : _connected = onConnected;

  final void Function() _connected;
  final List<List<dynamic>> sent = [];

  @override
  Future<void> connect() async {}

  @override
  void send(List<dynamic> payload) =>
      sent.add(jsonDecode(jsonEncode(payload)) as List<dynamic>);

  @override
  void dispose() {}
}

Future<({RelaySessionNotifier session, _SendingSocket socket})> _connected(
  RelayConfig config,
) async {
  final sockets = <_SendingSocket>[];
  final session = RelaySessionNotifier(
    socketFactory:
        ({
          required wsUrl,
          required nsec,
          required onMessage,
          required onConnected,
          required onDisconnected,
        }) {
          final socket = _SendingSocket(
            wsUrl: wsUrl,
            nsec: nsec,
            onMessage: onMessage,
            onConnected: onConnected,
            onDisconnected: onDisconnected,
          );
          sockets.add(socket);
          return socket;
        },
  );
  final container = ProviderContainer(
    overrides: [
      authProvider.overrideWith(() => _AuthenticatedAuthNotifier()),
      relaySessionProvider.overrideWith(() => session),
      relayConfigProvider.overrideWith(() => _ConfigNotifier(config)),
      relayAccessTokensProvider.overrideWithValue(FakeRelayAccessTokens()),
    ],
  );
  addTearDown(container.dispose);
  await container.read(authProvider.future);
  final sub = container.listen(relaySessionProvider, (_, _) {});
  addTearDown(sub.close);
  await Future<void>.delayed(Duration.zero);
  sockets.single._connected();
  await Future<void>.delayed(Duration.zero);
  return (session: session, socket: sockets.single);
}

String _nip01Id(Map<String, dynamic> event) => sha256
    .convert(
      utf8.encode(
        jsonEncode([
          0,
          event['pubkey'],
          event['created_at'],
          event['kind'],
          event['tags'],
          event['content'],
        ]),
      ),
    )
    .toString();

void main() {
  test('token mode submits an unsigned draft as the principal', () async {
    final h = await _connected(_tokenConfig);
    final relay = SignedEventRelay.forConfig(
      session: h.session,
      config: _tokenConfig,
    );
    expect(relay.pubkey, _principal);

    final result = relay.submit(
      kind: 9,
      content: 'hello',
      tags: const [
        ['h', 'channel-1'],
      ],
      createdAt: 1700000000,
    );
    await Future<void>.delayed(Duration.zero);

    final frame = h.socket.sent.single;
    expect(frame[0], 'EVENT');
    final draft = Map<String, dynamic>.from(frame[1] as Map);
    expect(draft.containsKey('sig'), isFalse);
    expect(draft['pubkey'], _principal);
    expect(draft['created_at'], 1700000000);
    expect(draft['id'], _nip01Id(draft));

    // The relay's OK carries the id it recomputed for the stamped event.
    h.session.debugHandleMessage(['OK', draft['id'], true, '']);
    expect((await result).id, draft['id']);
  });

  test('token mode never needs an nsec for any outgoing event', () {
    final event = buildOutgoingEvent(
      _tokenConfig,
      kind: 20002,
      content: '',
      tags: const [
        ['h', 'c'],
      ],
    );
    expect(event.sig, isNull);
    expect(event.pubkey, _principal);
    expect(outgoingAuthorPubkey(_tokenConfig), _principal);
  });

  test('legacy mode still signs with the nsec', () {
    final keys = nostr.Keys.generate();
    final config = RelayConfig(
      baseUrl: 'https://relay.example',
      nsec: keys.nsec,
    );
    final event = buildOutgoingEvent(
      config,
      kind: 9,
      content: 'x',
      tags: const [],
    );
    expect(event.pubkey, keys.public);
    expect(event.sig, isNotNull);
    expect(event.sig, isNotEmpty);
    expect(outgoingAuthorPubkey(config), keys.public);
  });
}

class _ConfigNotifier extends RelayConfigNotifier {
  _ConfigNotifier(this._config);
  final RelayConfig _config;

  @override
  RelayConfig build() => _config;
}

class _AuthenticatedAuthNotifier extends AuthNotifier {
  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.authenticated);
}
