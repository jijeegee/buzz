import 'dart:convert';

import 'package:buzz/features/invites/invite_create_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;

import '../../shared/relay/fake_relay_access_tokens.dart';

http.Response _minted() => http.Response(
  jsonEncode({
    'code': 'invite-code',
    'expires_at': 12345,
    'url': 'https://relay.example.com/invite/invite-code',
    'max_uses': null,
    'uses_remaining': null,
  }),
  200,
);

void main() {
  test(
    'a token community mints with Bearer and retries one token_expired',
    () async {
      final requests = <http.Request>[];
      final responses = [
        http.Response(
          jsonEncode({
            'error': 'authentication failed',
            'code': 'token_expired',
          }),
          401,
        ),
        _minted(),
      ];
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_2'],
      );
      final container = ProviderContainer(
        overrides: [
          relayConfigProvider.overrideWith(_TokenConfigNotifier.new),
          relayAccessTokensProvider.overrideWithValue(tokens),
          relaySessionProvider.overrideWith(_IdleRelaySession.new),
          communityInviteHttpClientProvider.overrideWithValue(
            http_testing.MockClient((request) async {
              requests.add(request);
              return responses.removeAt(0);
            }),
          ),
        ],
      );
      addTearDown(container.dispose);

      final invite = await container
          .read(communityInviteActionsProvider)
          .mintInvite(ttlSeconds: 86400, maxUses: null);

      expect(invite.code, 'invite-code');
      expect(requests.map((r) => r.headers['Authorization']), [
        'Bearer bzs_1',
        'Bearer bzs_2',
      ]);
      expect(
        requests.last.url,
        Uri.parse('https://relay.example.com/api/invites'),
      );
      expect(jsonDecode(requests.last.body), {'ttl_secs': 86400});
    },
  );

  test(
    'the invites 401 shape {"error":"token_expired"} also rotates once',
    () async {
      // invites.rs `invite_bearer` answers through `api_error`, which has no
      // `code` field: `{"error": "token_expired"}`.
      final requests = <http.Request>[];
      final responses = [
        http.Response(jsonEncode({'error': 'token_expired'}), 401),
        _minted(),
      ];
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_2'],
      );
      final container = ProviderContainer(
        overrides: [
          relayConfigProvider.overrideWith(_TokenConfigNotifier.new),
          relayAccessTokensProvider.overrideWithValue(tokens),
          relaySessionProvider.overrideWith(_IdleRelaySession.new),
          communityInviteHttpClientProvider.overrideWithValue(
            http_testing.MockClient((request) async {
              requests.add(request);
              return responses.removeAt(0);
            }),
          ),
        ],
      );
      addTearDown(container.dispose);

      final invite = await container
          .read(communityInviteActionsProvider)
          .mintInvite(ttlSeconds: 86400, maxUses: null);

      expect(invite.code, 'invite-code');
      expect(tokens.expiredCalls, ['bzs_1']);
      expect(requests.map((r) => r.headers['Authorization']), [
        'Bearer bzs_1',
        'Bearer bzs_2',
      ]);
    },
  );
}

class _IdleRelaySession extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
}

class _TokenConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://relay.example.com',
    tokenAuth: true,
    principalId:
        'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22',
  );
}
