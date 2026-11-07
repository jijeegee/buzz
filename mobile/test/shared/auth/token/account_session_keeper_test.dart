import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:kuzz/shared/relay/relay.dart';

import '../../community/community_storage_test.dart' show FakeSecureStorage;
import 'token_auth_test_fakes.dart';

const _origin = 'https://relay.test';
final _first = 'a' * 64;
final _second = 'b' * 64;
final _third = 'c' * 64;

class _Lifecycle extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;

  void go(AppLifecycleState next) => state = next;
}

Future<void> _settle() async {
  for (var i = 0; i < 20; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  late FakeAuthServer server;
  late FakeRefreshTokenStore tokens;
  late CommunityStorage storage;
  late FakeClock clock;
  late List<String> rotated;
  late ProviderContainer container;

  Community account(String pubkey, {bool signedOut = false}) {
    final community = Community.create(
      name: pubkey.substring(0, 1),
      relayUrl: _origin,
      pubkey: pubkey,
      tokenAuth: true,
    );
    return signedOut ? community.copyWith(signedOut: true) : community;
  }

  Future<void> seed(Community community) async {
    await storage.save(community);
    tokens.data[SessionKey(_origin, community.pubkey!)] = StoredTokenSession(
      refreshToken: 'bzr_${community.pubkey!.substring(0, 1)}',
      principalId: community.pubkey!,
    );
  }

  /// Answers rotations and records whose refresh token each one spent.
  void answerRotations(int count) {
    for (var i = 0; i < count; i++) {
      server.refreshResponses.add((request) {
        final spent = RegExp('bzr_[a-z0-9]+').firstMatch(request.body)!;
        rotated.add(spent.group(0)!);
        return FakeAuthServer.rotated('bzs_$i', 'bzr_next$i');
      });
    }
  }

  void open() {
    container = ProviderContainer(
      overrides: [
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        authHttpClientProvider.overrideWithValue(server.client),
        refreshTokenStoreProvider.overrideWithValue(tokens),
        keyBackupRefreshTokenStoreProvider.overrideWithValue(
          FakeRefreshTokenStore(),
        ),
        installIdStoreProvider.overrideWithValue(
          InstallIdStore(storage: FakeSecureStorage()),
        ),
        webAuthLauncherProvider.overrideWithValue(
          FakeWebAuthLauncher(FakeWebAuthLauncher.success),
        ),
        sessionClockProvider.overrideWithValue(clock.call),
        sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
        authDeviceNameProvider.overrideWithValue('Test phone'),
        appLifecycleProvider.overrideWith(_Lifecycle.new),
      ],
    );
    addTearDown(container.dispose);
    final sub = container.listen(accountSessionKeeperProvider, (_, _) {});
    addTearDown(sub.close);
  }

  setUp(() {
    server = FakeAuthServer();
    tokens = FakeRefreshTokenStore();
    storage = CommunityStorage(secure: FakeSecureStorage());
    clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
    rotated = [];
  });

  test('every signed-in account restores, active first, one at a time, '
      'and keeps its own timer', () async {
    final first = account(_first);
    final second = account(_second);
    final third = account(_third);
    await seed(first);
    await seed(second);
    await seed(third);
    await storage.saveActiveId(second.id);
    answerRotations(3);
    server.holdRefreshes = false;

    open();
    await _settle();

    expect(rotated.first, 'bzr_b');
    expect(rotated.toSet(), {'bzr_a', 'bzr_b', 'bzr_c'});
    expect(clock.active, hasLength(3));
    for (final pubkey in [_first, _second, _third]) {
      expect(
        container
            .read(tokenSessionProvider(SessionKey(_origin, pubkey)))
            .status,
        TokenSessionStatus.signedIn,
      );
    }
  });

  test('a signed-out account is left alone', () async {
    final active = account(_first);
    await seed(active);
    await seed(account(_second, signedOut: true));
    await storage.saveActiveId(active.id);
    answerRotations(1);

    open();
    await _settle();

    expect(rotated, ['bzr_a']);
  });

  test('a rebuild during the resume pass leaves no account behind', () async {
    final active = account(_first);
    await seed(active);
    await seed(account(_second));
    await seed(account(_third));
    await storage.saveActiveId(active.id);
    answerRotations(3);
    open();
    await _settle();
    expect(server.refreshCalls, 3);

    final lifecycle =
        container.read(appLifecycleProvider.notifier) as _Lifecycle;
    lifecycle.go(AppLifecycleState.paused);
    clock.now = clock.now.add(const Duration(minutes: 56));
    server.holdRefreshes = true;
    lifecycle.go(AppLifecycleState.resumed);
    await _settle();
    expect(server.heldRefreshes, hasLength(1), reason: 'one at a time');

    // Push bookkeeping and renames rebuild the keeper mid-pass.
    container.invalidate(activeCommunityProvider);
    await _settle();
    for (var i = 0; i < 2; i++) {
      server.heldRefreshes.removeFirst().complete(
        FakeAuthServer.rotated('bzs_late$i', 'bzr_late$i'),
      );
      await _settle();
    }
    expect(server.heldRefreshes, hasLength(1));
    server.heldRefreshes.removeFirst().complete(
      FakeAuthServer.rotated('bzs_late2', 'bzr_late2'),
    );
    await _settle();

    expect(server.refreshCalls, 6);
  });

  test('coming back refreshes each account near expiry', () async {
    final active = account(_first);
    final other = account(_second);
    await seed(active);
    await seed(other);
    await storage.saveActiveId(active.id);
    answerRotations(2);
    open();
    await _settle();
    expect(server.refreshCalls, 2);

    // Timers do not run in the background: time passes without them.
    final lifecycle =
        container.read(appLifecycleProvider.notifier) as _Lifecycle;
    lifecycle.go(AppLifecycleState.paused);
    clock.now = clock.now.add(const Duration(minutes: 56));
    answerRotations(2);
    lifecycle.go(AppLifecycleState.resumed);
    await _settle();

    expect(server.refreshCalls, 4);
  });
}
