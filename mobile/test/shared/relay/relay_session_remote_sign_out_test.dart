import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/auth/auth_provider.dart';
import 'package:kuzz/shared/auth/remote_sign_out.dart';
import 'package:kuzz/shared/auth/token/session_key.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:nostr/nostr.dart' as nostr;

const _origin = 'https://relay.example';
final _first = 'a' * 64;
final _second = 'b' * 64;

class _Socket extends RelaySocket {
  _Socket({
    required super.wsUrl,
    required super.nsec,
    required super.onMessage,
    required super.onConnected,
    required super.onDisconnected,
  }) : refuse = onDisconnected;

  final void Function(Object? error) refuse;

  @override
  Future<void> connect() async {}

  @override
  void dispose() {}
}

class _Auth extends AuthNotifier {
  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.authenticated);
}

class _Config extends RelayConfigNotifier {
  _Config(this.config);

  final RelayConfig config;

  @override
  RelayConfig build() => config;
}

class _Rows extends CommunityListNotifier {
  _Rows(this.rows);

  final List<Community> rows;

  @override
  Future<List<Community>> build() async => rows;
}

void main() {
  test('a refusal on an old socket checks the account that socket was for, '
      'after a switch to another account on the same server', () async {
    // Key accounts: a refused token socket waits on its token session.
    final keys = nostr.Keys.generate();
    final before = Community.create(
      name: 'First',
      relayUrl: _origin,
      pubkey: keys.public,
      nsec: keys.nsec,
      googleBackupAccountId: _first,
    );
    final after = Community.create(
      name: 'Second',
      relayUrl: _origin,
      pubkey: nostr.Keys.generate().public,
      googleBackupAccountId: _second,
    );
    final sockets = <_Socket>[];
    final checks = <(String, SessionKey)>[];
    final container = ProviderContainer(
      overrides: [
        authProvider.overrideWith(_Auth.new),
        communityListProvider.overrideWith(() => _Rows([before, after])),
        // The switch already happened; only the old socket is still around.
        activeCommunityProvider.overrideWith((ref) async => after),
        relayConfigProvider.overrideWith(
          () => _Config(
            RelayConfig(
              baseUrl: _origin,
              nsec: keys.nsec,
              communityId: before.id,
            ),
          ),
        ),
        relayAccessTokensProvider.overrideWithValue(null),
        relaySessionProvider.overrideWith(
          () => RelaySessionNotifier(
            socketFactory:
                ({
                  required wsUrl,
                  required nsec,
                  required onMessage,
                  required onConnected,
                  required onDisconnected,
                }) {
                  final socket = _Socket(
                    wsUrl: wsUrl,
                    nsec: nsec,
                    onMessage: onMessage,
                    onConnected: onConnected,
                    onDisconnected: onDisconnected,
                  );
                  sockets.add(socket);
                  return socket;
                },
          ),
        ),
        remoteSignOutCheckProvider.overrideWithValue(({
          required communityId,
          required session,
        }) async {
          checks.add((communityId, session));
        }),
      ],
    );
    addTearDown(container.dispose);
    await container.read(authProvider.future);
    await container.read(communityListProvider.future);
    await container.read(activeCommunityProvider.future);
    final sub = container.listen(relaySessionProvider, (_, _) {});
    addTearDown(sub.close);
    await Future<void>.delayed(Duration.zero);
    await Future<void>.delayed(Duration.zero);

    sockets.single.refuse(
      const RelayAuthRejectedException(relaySignedOutReason),
    );
    await Future<void>.delayed(Duration.zero);

    expect(checks, [(before.id, SessionKey(_origin, _first))]);
  });
}
