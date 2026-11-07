import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/auth/auth_provider.dart';
import 'package:kuzz/shared/auth/remote_sign_out.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:kuzz/shared/relay/relay_socket.dart';

import 'token/token_auth_test_fakes.dart';

final _session = SessionKey('https://relay.test', 'p');

class _RecordingAuth extends AuthNotifier {
  final signOuts = <bool>[];
  final communitySignOuts = <(String, bool)>[];
  final fences = <bool Function()?>[];

  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.authenticated);

  @override
  Future<void> signOut({bool deviceOnly = false}) async {
    signOuts.add(deviceOnly);
  }

  @override
  Future<void> signOutCommunity(
    String communityId, {
    bool deviceOnly = false,
    bool Function()? isCurrent,
  }) async {
    communitySignOuts.add((communityId, deviceOnly));
    fences.add(isCurrent);
  }
}

class _FakeAccountApi implements AccountApi {
  _FakeAccountApi(this.answer);

  final Future<AccountProfile> Function() answer;
  int calls = 0;

  @override
  Future<AccountProfile> me() {
    calls++;
    return answer();
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

void main() {
  late _RecordingAuth auth;

  ProviderContainer container(_FakeAccountApi api) {
    auth = _RecordingAuth();
    final container = ProviderContainer(
      overrides: [
        authProvider.overrideWith(() => auth),
        accountSessionProvider.overrideWithValue(_session),
        activeGoogleKeySessionProvider.overrideWithValue(_session),
        accountApiProvider(_session).overrideWithValue(api),
        keyBackupRefreshTokenStoreProvider.overrideWithValue(
          FakeRefreshTokenStore(),
        ),
        authHttpClientProvider.overrideWithValue(FakeAuthServer().client),
      ],
    );
    addTearDown(container.dispose);
    return container;
  }

  test('a confirmed remote sign-out signs out on this phone', () async {
    final api = _FakeAccountApi(
      () async => throw const AccountApiException(
        'revoked',
        statusCode: 401,
        code: 'token_revoked',
      ),
    );
    final c = container(api);
    await c.read(authProvider.future);
    await c.read(remoteSignOutCheckProvider)(
      communityId: 'community-a',
      session: _session,
      isCurrent: () => true,
    );
    expect(api.calls, 1);
    // Only the refused community, never every account.
    expect(auth.communitySignOuts, [('community-a', true)]);
    expect(auth.signOuts, isEmpty);
  });

  test('the sign-out carries the fence of the session asked about', () async {
    var current = true;
    final api = _FakeAccountApi(
      () async => throw const AccountApiException(
        'revoked',
        statusCode: 401,
        code: 'token_revoked',
      ),
    );
    final c = container(api);
    await c.read(authProvider.future);
    await c.read(remoteSignOutCheckProvider)(
      communityId: 'community-a',
      session: _session,
      isCurrent: () => current,
    );
    expect(auth.fences.single!(), isTrue);
    current = false;
    expect(auth.fences.single!(), isFalse);
  });

  test('a live account session keeps this phone signed in', () async {
    final api = _FakeAccountApi(
      () async => const AccountProfile(principalId: 'p', displayName: 'P'),
    );
    final c = container(api);
    await c.read(authProvider.future);
    await c.read(remoteSignOutCheckProvider)(
      communityId: 'community-a',
      session: _session,
      isCurrent: () => true,
    );
    expect(auth.communitySignOuts, isEmpty);
  });

  test('an unreachable relay or another refusal changes nothing', () async {
    for (final error in [
      const AccountApiException('Could not reach the relay'),
      const AccountApiException(
        'proof',
        statusCode: 401,
        code: 'device_proof_required',
      ),
      // Signed out locally only: the relay has not said the session ended.
      const AccountApiException('You are signed out of this relay.'),
      // A session-ended code without a 401 is not a verdict either.
      const AccountApiException(
        'revoked',
        statusCode: 403,
        code: 'token_revoked',
      ),
    ]) {
      final api = _FakeAccountApi(() async => throw error);
      final c = container(api);
      await c.read(authProvider.future);
      await c.read(remoteSignOutCheckProvider)(
        communityId: 'community-a',
        session: _session,
        isCurrent: () => true,
      );
      expect(auth.communitySignOuts, isEmpty);
    }
  });

  test('only the signed_out refusal triggers the check', () {
    expect(
      isRelaySignedOut(const RelayAuthRejectedException(relaySignedOutReason)),
      isTrue,
    );
    expect(
      isRelaySignedOut(const RelayAuthRejectedException('restricted: device')),
      isFalse,
    );
    expect(isRelaySignedOut(Exception(relaySignedOutReason)), isFalse);
  });
}
