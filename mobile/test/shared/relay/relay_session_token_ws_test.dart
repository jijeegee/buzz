import 'package:buzz/shared/auth/auth_provider.dart';
import 'package:buzz/shared/auth/token/token_session.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'fake_relay_access_tokens.dart';

const _principal =
    'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22';

class _TokenSocket extends RelaySocket {
  _TokenSocket({
    required super.wsUrl,
    required super.nsec,
    required super.onMessage,
    required super.onConnected,
    required super.onDisconnected,
  }) : _connected = onConnected,
       _disconnected = onDisconnected;

  final void Function() _connected;
  final void Function(Object? error) _disconnected;
  final List<String> reauths = [];
  RelayAccessTokens? tokensAtConnect;
  bool? refreshAtConnect;
  int disposeCalls = 0;

  @override
  Future<void> connect() async {
    tokensAtConnect = accessTokens;
    refreshAtConnect = refreshRejectedToken;
  }

  @override
  void reauthenticate(String token) => reauths.add(token);

  @override
  void dispose() => disposeCalls++;

  void connectSuccessfully() => _connected();
  void disconnectWith(Object? error) => _disconnected(error);
}

({ProviderContainer container, List<_TokenSocket> sockets, List<String?> nsecs})
_harness(FakeRelayAccessTokens tokens) {
  final sockets = <_TokenSocket>[];
  final nsecs = <String?>[];
  final session = RelaySessionNotifier(
    socketFactory:
        ({
          required wsUrl,
          required nsec,
          required onMessage,
          required onConnected,
          required onDisconnected,
        }) {
          final socket = _TokenSocket(
            wsUrl: wsUrl,
            nsec: nsec,
            onMessage: onMessage,
            onConnected: onConnected,
            onDisconnected: onDisconnected,
          );
          sockets.add(socket);
          nsecs.add(nsec);
          return socket;
        },
  );
  final container = ProviderContainer(
    overrides: [
      authProvider.overrideWith(() => _AuthenticatedAuthNotifier()),
      relaySessionProvider.overrideWith(() => session),
      relayConfigProvider.overrideWith(_TokenConfigNotifier.new),
      relayAccessTokensProvider.overrideWithValue(tokens),
    ],
  );
  addTearDown(container.dispose);
  return (container: container, sockets: sockets, nsecs: nsecs);
}

Future<void> _settle() => Future<void>.delayed(Duration.zero);

void main() {
  test('a token community auto-connects without an nsec', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    final h = _harness(tokens);
    await h.container.read(authProvider.future);
    final sub = h.container.listen(relaySessionProvider, (_, _) {});
    addTearDown(sub.close);
    await _settle();

    expect(h.sockets, hasLength(1));
    expect(h.nsecs.single, isNull);
    expect(h.sockets.single.tokensAtConnect, same(tokens));
    h.sockets.single.connectSuccessfully();
    await _settle();
    expect(
      h.container.read(relaySessionProvider).status,
      SessionStatus.connected,
    );
  });

  test('a rotation re-AUTHs the live socket instead of reconnecting', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    final h = _harness(tokens);
    await h.container.read(authProvider.future);
    final sub = h.container.listen(relaySessionProvider, (_, _) {});
    addTearDown(sub.close);
    await _settle();
    h.sockets.single.connectSuccessfully();
    await _settle();

    tokens.rotate('bzs_2');

    expect(h.sockets, hasLength(1));
    expect(h.sockets.single.reauths, ['bzs_2']);
  });

  test('no token parks the session until one is issued', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    final h = _harness(tokens);
    await h.container.read(authProvider.future);
    final sub = h.container.listen(relaySessionProvider, (_, _) {});
    addTearDown(sub.close);
    await _settle();

    h.sockets.single.disconnectWith(const RelayTokenUnavailableException());
    await _settle();
    expect(
      h.container.read(relaySessionProvider).status,
      SessionStatus.disconnected,
    );
    // No backoff loop while signed out / stalled.
    await Future<void>.delayed(const Duration(milliseconds: 1200));
    expect(h.sockets, hasLength(1));

    tokens.rotate('bzs_2');
    await _settle();
    expect(h.sockets, hasLength(2));
    expect(h.sockets.last.tokensAtConnect, same(tokens));
  });

  test('a refreshed first-AUTH rejection reconnects at once on a new socket, '
      'and a second rejection parks (bounded)', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    final h = _harness(tokens);
    await h.container.read(authProvider.future);
    final sub = h.container.listen(relaySessionProvider, (_, _) {});
    addTearDown(sub.close);
    await _settle();
    expect(h.sockets.single.refreshAtConnect, isTrue);

    h.sockets.single.disconnectWith(const RelayTokenRefreshedException());
    await _settle();

    expect(h.sockets, hasLength(2));
    expect(h.sockets.last.refreshAtConnect, isFalse);

    h.sockets.last.disconnectWith(
      const RelayAuthRejectedException('auth-required: token_expired'),
    );
    await Future<void>.delayed(const Duration(milliseconds: 1200));
    expect(h.sockets, hasLength(2));
    expect(
      h.container.read(relaySessionProvider).status,
      SessionStatus.disconnected,
    );

    // The next issued token recovers, with the refresh budget restored.
    tokens.rotate('bzs_3');
    await _settle();
    expect(h.sockets, hasLength(3));
    expect(h.sockets.last.refreshAtConnect, isTrue);
  });

  test(
    'a rotation while the socket is authenticating is handed to it',
    () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final h = _harness(tokens);
      await h.container.read(authProvider.future);
      final sub = h.container.listen(relaySessionProvider, (_, _) {});
      addTearDown(sub.close);
      await _settle();

      // Not yet connected: the socket queues it until AUTH OK.
      tokens.rotate('bzs_2');

      expect(h.sockets, hasLength(1));
      expect(h.sockets.single.reauths, ['bzs_2']);
    },
  );

  test('sign-out drops the socket and never reconnects', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    final h = _harness(tokens);
    await h.container.read(authProvider.future);
    final sub = h.container.listen(relaySessionProvider, (_, _) {});
    addTearDown(sub.close);
    await _settle();
    h.sockets.single.connectSuccessfully();
    await _settle();

    tokens.emitState(
      const TokenSessionState(status: TokenSessionStatus.signedOut),
    );
    await _settle();

    expect(h.sockets.single.disposeCalls, greaterThan(0));
    expect(
      h.container.read(relaySessionProvider).status,
      SessionStatus.disconnected,
    );
    await Future<void>.delayed(const Duration(milliseconds: 1200));
    expect(h.sockets, hasLength(1));
  });

  test('disposing the session removes its token listeners', () async {
    final tokens = FakeRelayAccessTokens(token: 'bzs_1');
    final h = _harness(tokens);
    await h.container.read(authProvider.future);
    final sub = h.container.listen(relaySessionProvider, (_, _) {});
    await _settle();
    expect(tokens.tokenListenerCount, 1);
    expect(tokens.stateListenerCount, 1);

    sub.close();
    h.container.dispose();

    expect(tokens.tokenListenerCount, 0);
    expect(tokens.stateListenerCount, 0);
  });

  test('a token community without a canonical origin has no token source '
      '(null, never a FormatException)', () {
    final container = ProviderContainer(
      overrides: [
        relayConfigProvider.overrideWith(_BadOriginConfigNotifier.new),
      ],
    );
    addTearDown(container.dispose);

    expect(container.read(relayAccessTokensProvider), isNull);
  });
}

class _BadOriginConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'ftp://relay.example',
    tokenAuth: true,
    principalId: _principal,
  );
}

class _TokenConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://relay.example',
    tokenAuth: true,
    principalId: _principal,
  );
}

class _AuthenticatedAuthNotifier extends AuthNotifier {
  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.authenticated);
}
