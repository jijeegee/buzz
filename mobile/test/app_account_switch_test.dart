import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/app.dart';
import 'package:kuzz/features/activity/activity_provider.dart';
import 'package:kuzz/features/activity/feed_item.dart';
import 'package:kuzz/features/activity/inbox_local_state_provider.dart';
import 'package:kuzz/features/activity/inbox_summary.dart';
import 'package:kuzz/features/activity/inbox_summary_provider.dart';
import 'package:kuzz/features/age_gate/age_signal_provider.dart';
import 'package:kuzz/features/channels/channel_preload.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/features/forum/forum_provider.dart';
import 'package:kuzz/features/home/home_page.dart';
import 'package:kuzz/features/people/blocked_users_provider.dart';
import 'package:kuzz/features/profile/profile_provider.dart';
import 'package:kuzz/features/settings/settings_page.dart';
import 'package:kuzz/features/sign_in/token_sign_in_page.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'shared/auth/token/token_auth_test_fakes.dart';
import 'shared/community/community_storage_test.dart' show FakeSecureStorage;

const _origin = 'https://relay.test';
final _jiho = 'a1' * 32;
final _nodal = 'b2' * 32;
final _third = 'c3' * 32;

class _AllowedAgeSignalNotifier extends AgeSignalNotifier {
  @override
  AgeSignalState build() => AgeSignalState.allowed;

  @override
  Future<void> request() async {}
}

/// Keeps HomePage offline, and remembers who asked for what.
class _OfflineRelaySessionNotifier extends RelaySessionNotifier {
  static var historyFetches = 0;

  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    historyFetches++;
    return const [];
  }
}

Community _row(
  String principal, {
  required String name,
  String? email,
  DateTime? lastUsedAt,
  bool signedOut = false,
}) =>
    Community.create(
      name: 'relay',
      relayUrl: _origin,
      pubkey: principal,
      tokenAuth: true,
    ).copyWith(
      displayName: name,
      email: email,
      lastUsedAt: lastUsedAt,
      signedOut: signedOut,
    );

class _Harness {
  final server = FakeAuthServer();
  final tokens = FakeRefreshTokenStore();
  final storage = CommunityStorage(secure: FakeSecureStorage());
  final clock = FakeClock(DateTime.utc(2026, 11, 7, 12));
  late final ProviderContainer container;
  late Community jiho;
  late Community nodal;

  Future<void> seed(Community row) async {
    await storage.save(row);
    if (row.signedOut) return;
    tokens.data[SessionKey(_origin, row.pubkey!)] = StoredTokenSession(
      refreshToken: 'bzr_${row.displayName}',
      principalId: row.pubkey!,
    );
    server.refreshResponses.add(
      (_) => FakeAuthServer.rotated(
        'bzs_${row.displayName}',
        'bzr_${row.displayName}',
      ),
    );
  }

  /// Two accounts on one server, Jiho on screen.
  Future<void> start(
    WidgetTester tester, {
    List<Community> also = const [],
  }) async {
    jiho = _row(_jiho, name: 'Jiho', email: 'jiho@example.com');
    nodal = _row(_nodal, name: 'Nodal Studio', email: 'cs@example.com');
    await seed(jiho);
    await seed(nodal);
    for (final row in also) {
      await seed(row);
    }
    await storage.saveActiveId(jiho.id);
    _OfflineRelaySessionNotifier.historyFetches = 0;
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    container = ProviderContainer(
      retry: (_, _) => null,
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        ageSignalProvider.overrideWith(_AllowedAgeSignalNotifier.new),
        relaySessionProvider.overrideWith(_OfflineRelaySessionNotifier.new),
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        authHttpClientProvider.overrideWithValue(server.client),
        refreshTokenStoreProvider.overrideWithValue(tokens),
        webAuthLauncherProvider.overrideWithValue(
          FakeWebAuthLauncher(FakeWebAuthLauncher.success),
        ),
        sessionClockProvider.overrideWithValue(clock.call),
        sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
        authDeviceNameProvider.overrideWithValue('Test phone'),
        installIdStoreProvider.overrideWithValue(
          InstallIdStore(storage: FakeSecureStorage()),
        ),
      ],
    );
    await tester.pumpWidget(
      UncontrolledProviderScope(container: container, child: const App()),
    );
    await frames(tester);
  }

  /// Settings over Home, as the picture at the top opens it.
  Future<void> openSettings(WidgetTester tester) async {
    final home = find.byType(HomePage);
    unawaited(
      Navigator.of(tester.element(home), rootNavigator: true).push(
        MaterialPageRoute<void>(
          builder: tester.widget<HomePage>(home).settingsPageBuilder,
        ),
      ),
    );
    await settle(tester);
  }

  Future<void> tapRow(WidgetTester tester, Community row) async {
    await tester.tap(find.byKey(ValueKey('settings-account-${row.id}')));
    await settle(tester);
  }

  Future<void> addAccount(WidgetTester tester) async {
    await tester.tap(find.byKey(const Key('settings-add-account')));
    await settle(tester);
    await tester.enterText(
      find.byKey(const Key('token-sign-in-relay-url')),
      'relay.test',
    );
    await tester.tap(find.byKey(const Key('token-sign-in-check')));
    await settle(tester);
  }

  void relayAnswersAs(String principal) {
    server.complete = (_) => jsonResponse({
      'principal_id': principal,
      'device_id': 'device-1',
      'access': 'bzs_login',
      'refresh': 'bzr_login',
      'expires_in': 3600,
    });
  }

  String? get principalOnScreen => container.read(myPubkeyProvider);

  Future<void> unmount(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox.shrink());
    container.dispose();
    await tester.pump(const Duration(seconds: 10));
  }
}

Future<void> frames(WidgetTester tester) async {
  for (var i = 0; i < 10; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

/// Past a page's transition.
Future<void> settle(WidgetTester tester) async {
  await frames(tester);
  await tester.pump(const Duration(seconds: 1));
  await frames(tester);
}

void main() {
  testWidgets('a switch on one server shows only that account, from Home', (
    tester,
  ) async {
    final h = _Harness();
    await h.start(tester);
    h.container.read(inboxLocalStateProvider.notifier).markDone('from-jiho');
    final firstHome = tester.state(find.byType(HomePage));
    await h.openSettings(tester);

    await h.tapRow(tester, h.nodal);

    expect(find.byType(SettingsPage), findsNothing, reason: 'stack popped');
    expect(find.byType(HomePage), findsOneWidget);
    expect(tester.state(find.byType(HomePage)), isNot(same(firstHome)));
    expect(h.principalOnScreen, _nodal);
    expect(await h.storage.loadActiveId(), h.nodal.id);
    expect(h.container.read(inboxLocalStateProvider).doneIds, isEmpty);
    expect(h.container.read(authProvider).value?.community?.id, h.nodal.id);

    await h.openSettings(tester);
    await h.tapRow(tester, h.jiho);

    expect(find.byType(SettingsPage), findsNothing);
    expect(h.principalOnScreen, _jiho);
    expect(h.container.read(inboxLocalStateProvider).doneIds, {'from-jiho'});
    await h.unmount(tester);
  });

  testWidgets('nothing of one account stays in the next one\'s providers', (
    tester,
  ) async {
    final h = _Harness();
    await h.start(tester);
    const thread = ThreadRepliesArgs(channelId: 'c', rootId: 'r');
    h.container.read(preloadedThreadsProvider.notifier).seed(thread, const []);
    final forum = h.container.listen(forumPostsProvider('c'), (_, _) {});
    addTearDown(forum.close);
    await frames(tester);
    final before = _OfflineRelaySessionNotifier.historyFetches;
    expect(h.container.read(preloadedThreadsProvider), isNotEmpty);

    await h.openSettings(tester);
    await h.tapRow(tester, h.nodal);

    expect(h.container.read(preloadedThreadsProvider), isEmpty);
    forum.read();
    await frames(tester);
    expect(_OfflineRelaySessionNotifier.historyFetches, greaterThan(before));
    await h.unmount(tester);
  });

  testWidgets('values kept while reloading do not reach the next account', (
    tester,
  ) async {
    final h = _Harness();
    await h.start(tester);
    // Everything shown under Nodal, loading states included.
    final shown = <Object?>[];
    void record(Object? value) {
      if (h.principalOnScreen == _nodal) shown.add(value);
    }

    final subscriptions = [
      h.container.listen(activityProvider, (_, next) => record(next.value)),
      h.container.listen(inboxSummaryProvider, (_, _) {}),
      h.container.listen(profileProvider, (_, next) => record(next.value)),
      h.container.listen(blockedUsersProvider, (_, _) {}),
    ];
    addTearDown(() {
      for (final subscription in subscriptions) {
        subscription.close();
      }
    });
    await frames(tester);
    final feed = HomeFeedResponse(
      mentions: const [],
      needsAction: const [],
      activity: const [],
      agentActivity: const [],
    );
    const rows = InboxSummaryState(rows: [], next: null, sentRead: {'c': 1});
    // ignore_for_file: invalid_use_of_protected_member
    h.container.read(activityProvider.notifier).state = AsyncData(feed);
    h.container.read(inboxSummaryProvider.notifier).state = const AsyncData(
      rows,
    );
    final profile = UserProfile(pubkey: _jiho, displayName: 'Jiho');
    h.container.read(profileProvider.notifier).state = AsyncData(profile);
    h.container.read(blockedUsersProvider.notifier).state = const AsyncData({
      'blocked-by-jiho',
    });

    await h.openSettings(tester);
    await h.tapRow(tester, h.nodal);

    expect(shown, isNot(contains(same(feed))));
    expect(shown, isNot(contains(same(profile))));
    expect(h.container.read(inboxSummaryProvider).value?.sentRead, isEmpty);
    expect(
      h.container.read(blockedUsersProvider).value ?? const <String>{},
      isNot(contains('blocked-by-jiho')),
    );
    await h.unmount(tester);
  });

  testWidgets('a fallback to another account also starts from Home', (
    tester,
  ) async {
    final h = _Harness();
    await h.start(tester);
    await h.openSettings(tester);

    // As a remote sign-out does: from outside the screen.
    await h.container
        .read(authProvider.notifier)
        .signOutCommunity(h.jiho.id, deviceOnly: true);
    await settle(tester);

    expect(find.byType(SettingsPage), findsNothing);
    expect(find.byType(HomePage), findsOneWidget);
    expect(h.principalOnScreen, _nodal);
    await h.unmount(tester);
  });

  group('the account list', () {
    testWidgets('names accounts, marks the active one, hides signed-out ones', (
      tester,
    ) async {
      final h = _Harness();
      final gone = _row(_third, name: 'Gone', signedOut: true);
      await h.start(tester, also: [gone]);
      await h.openSettings(tester);

      expect(find.text('Accounts'), findsOneWidget);
      expect(find.text('Nodal Studio'), findsOneWidget);
      expect(find.text('cs@example.com'), findsOneWidget);
      expect(find.text('Gone'), findsNothing);
      expect(find.byKey(ValueKey('settings-account-${gone.id}')), findsNothing);
      expect(
        find.descendant(
          of: find.byKey(ValueKey('settings-account-${h.jiho.id}')),
          matching: find.byKey(const Key('settings-account-active')),
        ),
        findsOneWidget,
      );
      expect(find.byKey(const Key('settings-account-active')), findsOneWidget);
      expect(find.textContaining('relay'), findsNothing);
      await h.unmount(tester);
    });

    testWidgets('lists the most recently used first, Add account last', (
      tester,
    ) async {
      final h = _Harness();
      final used = _row(
        _third,
        name: 'Third',
        lastUsedAt: DateTime.utc(2026, 11, 1),
      );
      await h.start(tester, also: [used]);
      await h.openSettings(tester);

      double top(Key key) => tester.getTopLeft(find.byKey(key)).dy;
      final third = top(ValueKey('settings-account-${used.id}'));
      final jiho = top(ValueKey('settings-account-${h.jiho.id}'));
      final nodal = top(ValueKey('settings-account-${h.nodal.id}'));
      expect(third, lessThan(jiho));
      expect(jiho, lessThan(nodal));
      expect(nodal, lessThan(top(const Key('settings-add-account'))));
      await h.unmount(tester);
    });
  });

  group('sign-out', () {
    Future<void> openDialog(WidgetTester tester) async {
      final row = find.byKey(const Key('settings-sign-out'));
      await tester.scrollUntilVisible(
        row,
        200,
        scrollable: find
            .descendant(
              of: find.byType(SettingsPage),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      await tester.pump();
      await tester.tap(row);
      await settle(tester);
    }

    testWidgets('names the account and who stays signed in', (tester) async {
      final h = _Harness();
      await h.start(tester);
      await h.openSettings(tester);

      await openDialog(tester);

      expect(find.text('Sign out of Jiho'), findsOneWidget);
      expect(find.text('Sign out of Jiho?'), findsOneWidget);
      expect(
        find.text('Nodal Studio stays signed in on this phone.'),
        findsOneWidget,
      );
      await h.unmount(tester);
    });

    testWidgets('counts the others when there are several', (tester) async {
      final h = _Harness();
      await h.start(tester, also: [_row(_third, name: 'Third')]);
      await h.openSettings(tester);

      await openDialog(tester);

      expect(
        find.text('Nodal Studio and 1 other stay signed in on this phone.'),
        findsOneWidget,
      );
      await h.unmount(tester);
    });
  });

  group('Add account', () {
    testWidgets('Back returns to the account on screen, unchanged', (
      tester,
    ) async {
      final h = _Harness();
      await h.start(tester);
      await h.openSettings(tester);
      await h.addAccount(tester);
      expect(find.byType(TokenSignInPage), findsOneWidget);

      await tester.tap(find.byType(BackButton));
      await settle(tester);

      expect(find.byType(TokenSignInPage), findsNothing);
      expect(find.byType(SettingsPage), findsOneWidget);
      expect(h.principalOnScreen, _jiho);
      expect(await h.storage.loadAll(), hasLength(2));
      await h.unmount(tester);
    });

    testWidgets('an account already here is switched to, session kept', (
      tester,
    ) async {
      final h = _Harness();
      await h.start(tester);
      final kept = h.tokens.data[SessionKey(_origin, _nodal)]?.refreshToken;
      await h.openSettings(tester);
      await h.addAccount(tester);
      h.relayAnswersAs(_nodal);

      await tester.tap(find.byKey(const Key('token-sign-in-google')));
      await settle(tester);

      expect(find.byType(TokenSignInPage), findsNothing);
      expect(find.byType(SettingsPage), findsNothing);
      expect(h.principalOnScreen, _nodal);
      expect(await h.storage.loadAll(), hasLength(2));
      expect(h.tokens.data[SessionKey(_origin, _nodal)]?.refreshToken, kept);
      expect(
        h.server.requests.where((r) => r.url.path == '/auth/sessions/end'),
        hasLength(1),
        reason: 'the extra sign-in ends',
      );
      await h.unmount(tester);
    });

    testWidgets('the account on screen just goes back', (tester) async {
      final h = _Harness();
      await h.start(tester);
      await h.openSettings(tester);
      await h.addAccount(tester);
      h.relayAnswersAs(_jiho);

      await tester.tap(find.byKey(const Key('token-sign-in-google')));
      await settle(tester);

      expect(find.byType(TokenSignInPage), findsNothing);
      expect(find.byType(SettingsPage), findsOneWidget);
      expect(h.principalOnScreen, _jiho);
      await h.unmount(tester);
    });

    testWidgets('a new account joins the rest and becomes active', (
      tester,
    ) async {
      final h = _Harness();
      await h.start(tester);
      await h.openSettings(tester);
      await h.addAccount(tester);
      h.relayAnswersAs(_third);

      await tester.tap(find.byKey(const Key('token-sign-in-google')));
      await settle(tester);

      expect(find.byType(TokenSignInPage), findsNothing);
      expect(find.byType(SettingsPage), findsNothing);
      expect(find.byType(HomePage), findsOneWidget);
      expect(h.principalOnScreen, _third);
      final rows = await h.storage.loadAll();
      expect(rows, hasLength(3));
      expect(rows.where((row) => row.signedOut), isEmpty);
      await h.unmount(tester);
    });
  });
}
