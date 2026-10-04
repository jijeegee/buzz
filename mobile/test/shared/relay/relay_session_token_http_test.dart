import 'dart:async';
import 'dart:convert';

import 'package:buzz/shared/auth/auth_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;

import 'fake_relay_access_tokens.dart';

const _principal =
    'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22';

http.Response _expired() => http.Response(
  jsonEncode({'error': 'authentication failed', 'code': 'token_expired'}),
  401,
);

({RelaySessionNotifier session, List<http.Request> requests}) _harness(
  FakeRelayAccessTokens tokens,
  List<http.Response> responses,
) {
  final requests = <http.Request>[];
  final session = RelaySessionNotifier(
    httpClient: http_testing.MockClient((request) async {
      requests.add(request);
      return responses.removeAt(0);
    }),
  );
  final container = ProviderContainer(
    overrides: [
      authProvider.overrideWith(() => _PendingAuthNotifier()),
      relaySessionProvider.overrideWith(() => session),
      relayConfigProvider.overrideWith(_TokenConfigNotifier.new),
      relayAccessTokensProvider.overrideWithValue(tokens),
    ],
  );
  addTearDown(container.dispose);
  container.read(relaySessionProvider);
  return (session: session, requests: requests);
}

void main() {
  group('queryRelay on a token community', () {
    test('sends the bearer token instead of NIP-98', () async {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final h = _harness(tokens, [http.Response('[]', 200)]);
      expect(
        await h.session.queryRelay(const [
          NostrFilter(kinds: [1]),
        ]),
        isEmpty,
      );
      expect(h.requests.single.url.path, '/query');
      expect(h.requests.single.headers['Authorization'], 'Bearer bzs_1');
    });

    test('a 401 token_expired rotates once and retries once', () async {
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_2'],
      );
      final h = _harness(tokens, [_expired(), http.Response('[]', 200)]);
      expect(
        await h.session.queryRelay(const [
          NostrFilter(kinds: [1]),
        ]),
        isEmpty,
      );
      expect(tokens.expiredCalls, ['bzs_1']);
      expect(h.requests.map((r) => r.headers['Authorization']), [
        'Bearer bzs_1',
        'Bearer bzs_2',
      ]);
    });

    test('a second token_expired is surfaced, not retried again', () async {
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_2', 'bzs_3'],
      );
      final h = _harness(tokens, [_expired(), _expired()]);
      await expectLater(
        h.session.queryRelay(const [
          NostrFilter(kinds: [1]),
        ]),
        throwsA(
          isA<RelayException>().having((e) => e.statusCode, 'status', 401),
        ),
      );
      expect(h.requests, hasLength(2));
      expect(tokens.expiredCalls, ['bzs_1']);
    });

    test('no token fails without touching the network', () async {
      final tokens = FakeRelayAccessTokens(token: null);
      final h = _harness(tokens, []);
      await expectLater(
        h.session.queryRelay(const [
          NostrFilter(kinds: [1]),
        ]),
        throwsA(isA<RelayTokenUnavailableException>()),
      );
      expect(h.requests, isEmpty);
    });
  });
}

class _TokenConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://relay.example',
    tokenAuth: true,
    principalId: _principal,
  );
}

class _PendingAuthNotifier extends AuthNotifier {
  @override
  Future<AuthState> build() => Completer<AuthState>().future;
}
