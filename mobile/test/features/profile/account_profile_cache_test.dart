import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/activity/inbox_local_state_provider.dart';
import 'package:kuzz/features/profile/account_profile_cache.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/accent_colors.dart';
import 'package:kuzz/shared/theme/community_theme_provider.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/community/community_storage_test.dart'
    show FakeSecureStorage;

const _origin = 'https://relay.test';
final _a = 'a1' * 32;
final _b = 'b2' * 32;

class _OfflineRelaySessionNotifier extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
}

Community _row(String principal) => Community.create(
  name: 'relay',
  relayUrl: _origin,
  pubkey: principal,
  tokenAuth: true,
);

class _Harness {
  _Harness(this.prefs);

  final SharedPreferences prefs;
  final storage = CommunityStorage(secure: FakeSecureStorage());
  final tokens = FakeRefreshTokenStore();
  final server = FakeAuthServer();
  final clock = FakeClock(DateTime.utc(2026, 11, 7, 12));
  final profiles = <SessionKey, Completer<AccountProfile>>{};
  late final a = _row(_a);
  late final b = _row(_b);
  late final ProviderContainer container;

  static Future<_Harness> start({Map<String, Object> saved = const {}}) async {
    SharedPreferences.setMockInitialValues(saved);
    final h = _Harness(await SharedPreferences.getInstance());
    for (final row in [h.a, h.b]) {
      await h.storage.save(row);
      h.tokens.data[SessionKey(_origin, row.pubkey!)] = StoredTokenSession(
        refreshToken: 'bzr_${row.id}',
        principalId: row.pubkey!,
      );
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_${row.id}', 'bzr2_${row.id}'),
      );
    }
    await h.storage.saveActiveId(h.a.id);
    h.container = ProviderContainer(
      retry: (_, _) => null,
      overrides: [
        savedPrefsProvider.overrideWithValue(h.prefs),
        relaySessionProvider.overrideWith(_OfflineRelaySessionNotifier.new),
        communityStorageProvider.overrideWithValue(h.storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        authHttpClientProvider.overrideWithValue(h.server.client),
        refreshTokenStoreProvider.overrideWithValue(h.tokens),
        sessionClockProvider.overrideWithValue(h.clock.call),
        sessionTimerFactoryProvider.overrideWithValue(h.clock.createTimer),
        installIdStoreProvider.overrideWithValue(
          InstallIdStore(storage: FakeSecureStorage()),
        ),
        accountProfileProvider.overrideWith(
          (ref, key) => h.profiles.putIfAbsent(key, Completer.new).future,
        ),
      ],
    );
    addTearDown(h.container.dispose);
    h.container.listen(accountSessionKeeperProvider, (_, _) {});
    await h.container.read(authProvider.future);
    await h.container.read(activeCommunityProvider.future);
    await pumpEventQueue();
    return h;
  }

  Future<Community> saved(Community row) async =>
      (await storage.loadAll()).singleWhere((c) => c.id == row.id);

  Future<void> switchTo(Community row) async {
    await container
        .read(communityListProvider.notifier)
        .switchCommunity(row.id);
    await container.read(activeCommunityProvider.future);
    await pumpEventQueue();
  }
}

void main() {
  group('the row cache', () {
    test('takes the active account\'s profile when it loads', () async {
      final h = await _Harness.start();
      h.container.listen(accountProfileCacheProvider, (_, _) {});
      await pumpEventQueue();
      final key = SessionKey(_origin, _a);
      expect(h.profiles, contains(key));

      h.profiles[key]!.complete(
        AccountProfile(
          principalId: _a,
          displayName: 'Jiho',
          avatarUrl: 'https://a.example/j.png',
          email: 'jiho@example.com',
        ),
      );
      await pumpEventQueue();

      final row = await h.saved(h.a);
      expect(row.displayName, 'Jiho');
      expect(row.avatarUrl, 'https://a.example/j.png');
      expect(row.email, 'jiho@example.com');
      final list = h.container.read(communityListProvider).value!;
      expect(list.singleWhere((c) => c.id == h.a.id).email, row.email);
      final other = await h.saved(h.b);
      expect(other.displayName, isNull);
      expect(other.email, isNull);
    });

    test(
      'a profile arriving after the switch stays with its account',
      () async {
        final h = await _Harness.start();
        h.container.listen(accountProfileCacheProvider, (_, _) {});
        await pumpEventQueue();
        final late = h.profiles[SessionKey(_origin, _a)]!;

        await h.switchTo(h.b);
        late.complete(AccountProfile(principalId: _a, displayName: 'Jiho'));
        await pumpEventQueue();

        expect((await h.saved(h.b)).displayName, isNull);
      },
    );

    test('caching a profile leaves the live session alone', () async {
      final h = await _Harness.start();
      h.container.listen(accountProfileCacheProvider, (_, _) {});
      final config = h.container.read(relayConfigProvider);
      await pumpEventQueue();

      h.profiles[SessionKey(_origin, _a)]!.complete(
        AccountProfile(principalId: _a, displayName: 'Jiho'),
      );
      await pumpEventQueue();
      await h.container.read(activeCommunityProvider.future);

      expect(h.container.read(relayConfigProvider), same(config));
    });
  });

  group('inbox overrides', () {
    test('belong to one account, and rebuild on a switch', () async {
      final h = await _Harness.start();
      final inbox = h.container.listen(inboxLocalStateProvider, (_, _) {});
      addTearDown(inbox.close);
      h.container.read(inboxLocalStateProvider.notifier).markDone('row');

      await h.switchTo(h.b);
      expect(inbox.read().doneIds, isEmpty);
      h.container.read(inboxLocalStateProvider.notifier).markUnread(['other']);

      await h.switchTo(h.a);
      expect(inbox.read().doneIds, {'row'});
      expect(inbox.read().unreadIds, isEmpty);
    });

    test('the shared value of older versions is deleted', () async {
      final h = await _Harness.start(
        saved: {
          'activity_inbox_local_state_v1': '{"done":["old"],"unread":[]}',
        },
      );
      expect(h.container.read(inboxLocalStateProvider).doneIds, isEmpty);
      await pumpEventQueue();
      expect(h.prefs.containsKey('activity_inbox_local_state_v1'), isFalse);
    });
  });

  group('theme', () {
    test('a token account keeps its own, off the shared key', () async {
      final h = await _Harness.start();
      final theme = h.container.listen(communityThemeProvider, (_, _) {});
      addTearDown(theme.close);
      final legacy = h.prefs.getKeys().toSet();
      final notifier = h.container.read(communityThemeProvider.notifier);
      notifier.setAccent(
        accentColors[1].wireValue == theme.read().accent ? 2 : 1,
      );
      await pumpEventQueue();
      final chosen = theme.read().accent;
      expect(h.prefs.getKeys().difference(legacy), isNotEmpty);

      await h.switchTo(h.b);
      expect(theme.read().accent, isNot(chosen));

      await h.switchTo(h.a);
      expect(theme.read().accent, chosen);
    });
  });
}
