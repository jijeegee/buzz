import 'package:kuzz/features/settings/account_page.dart';
import 'package:kuzz/features/settings/devices_page.dart';
import 'package:kuzz/features/settings/settings_page.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../shared/auth/token/token_auth_test_fakes.dart';
import 'token_settings_harness.dart';

Widget _settings() => SettingsPage(
  profileHeader: const SizedBox.shrink(),
  invitePageBuilder: (_) => const SizedBox.shrink(),
);

Widget _profile() => SettingsPage(
  mode: SettingsPageMode.profile,
  profileHeader: const SizedBox.shrink(),
  invitePageBuilder: (_) => const SizedBox.shrink(),
);

/// Scrolls [finder] to the middle of the page, clear of the frosted bars.
Future<void> _centerInView(WidgetTester tester, Finder finder) async {
  await tester.scrollUntilVisible(finder, 200);
  await Scrollable.ensureVisible(tester.element(finder), alignment: 0.5);
  await tester.pumpAndSettle();
}

Future<void> _tapRemoveAndConfirm(WidgetTester tester) async {
  await _centerInView(tester, find.text('Remove community'));
  await tester.tap(find.text('Remove community'));
  await frames(tester);
  await tester.ensureVisible(
    find.byKey(const Key('settings-remove-community')),
  );
  await tester.tap(find.byKey(const Key('settings-remove-community')));
  await frames(tester);
  await tester.tap(find.widgetWithText(FilledButton, 'Remove'));
  await frames(tester);
}

void main() {
  group('Remove community (token)', () {
    testWidgets('signs the device out on the relay before removing', (
      tester,
    ) async {
      final h = TokenSettingsHarness();
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await h.pump(tester, _settings());

      await _tapRemoveAndConfirm(tester);

      expect(
        h.server.requests.where((r) => r.url.path == '/auth/logout'),
        hasLength(1),
      );
      expect(h.tokens.data, isEmpty);
      expect(await h.storage.loadAll(), isEmpty);
      expect(
        h.container.read(authProvider).value?.status,
        AuthStatus.unauthenticated,
      );
    });

    testWidgets(
      'an unreachable relay keeps the community and offers device-only removal',
      (tester) async {
        final h = TokenSettingsHarness();
        for (var i = 0; i < 4; i++) {
          h.server.refreshResponses.add(
            (_) => jsonResponse({'error': 'unavailable'}, status: 503),
          );
        }
        await h.pump(tester, _settings());

        await _tapRemoveAndConfirm(tester);

        // Surfaced, nothing removed.
        expect(find.textContaining('Could not reach the relay'), findsWidgets);
        expect((await h.storage.loadAll()).single.id, h.community.id);
        expect(h.tokens.data[tokenOrigin]?.refreshToken, 'bzr_old');

        await tester.tap(find.byKey(const Key('remove-community-device-only')));
        await frames(tester);

        expect(await h.storage.loadAll(), isEmpty);
        expect(h.tokens.data, isEmpty);
        expect(
          h.server.requests.where((r) => r.url.path == '/auth/logout'),
          isEmpty,
        );
      },
    );
  });

  testWidgets('sign-out asks first; cancel keeps the session', (tester) async {
    final h = TokenSettingsHarness();
    h.server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );
    await h.pump(tester, _settings());
    final signOut = find.byKey(const Key('settings-sign-out'));
    await tester.scrollUntilVisible(signOut, 200);
    await tester.ensureVisible(signOut);
    await tester.pumpAndSettle();
    expect(
      find.descendant(of: signOut, matching: find.text('Sign out')),
      findsOneWidget,
    );
    await tester.tap(signOut);
    await frames(tester);

    final dialog = find.byKey(const Key('settings-sign-out-dialog'));
    expect(dialog, findsOneWidget);
    expect(
      find.descendant(of: dialog, matching: find.text('Sign out?')),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: dialog,
        matching: find.textContaining('signed out of all your communities'),
      ),
      findsOneWidget,
    );
    await tester.tap(find.byKey(const Key('settings-sign-out-cancel')));
    // Past the dialog's 220ms exit transition.
    await tester.pump(const Duration(milliseconds: 300));
    await frames(tester);

    expect(dialog, findsNothing);
    expect((await h.storage.loadAll()).single.signedOut, isFalse);
    expect(h.tokens.data, isNotEmpty);
    expect(
      h.container.read(authProvider).value?.status,
      AuthStatus.authenticated,
    );
    expect(
      h.server.requests.where((r) => r.url.path == '/auth/logout'),
      isEmpty,
    );
  });

  testWidgets('confirmed sign-out ends the session and keeps the community', (
    tester,
  ) async {
    final h = TokenSettingsHarness();
    h.server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );
    await h.pump(tester, _settings());
    expect(
      find.byKey(const Key('settings-remove-community')).hitTestable(),
      findsNothing,
    );
    await tester.scrollUntilVisible(
      find.byKey(const Key('settings-sign-out')),
      200,
    );
    await tester.ensureVisible(find.byKey(const Key('settings-sign-out')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('settings-sign-out')));
    await frames(tester);
    // The confirmation is the sign-out dialog, not the removal dialog.
    expect(find.text('Remove community from this device?'), findsNothing);
    await tester.tap(find.byKey(const Key('settings-sign-out-confirm')));
    // Past the dialog's 220ms exit transition.
    await tester.pump(const Duration(milliseconds: 300));
    await frames(tester);
    expect(find.byType(AlertDialog), findsNothing);
    final saved = (await h.storage.loadAll()).single;
    expect(saved.id, h.community.id);
    expect(saved.pubkey, h.community.pubkey);
    expect(saved.signedOut, isTrue);
    expect(h.tokens.data, isEmpty);
    expect(
      h.container.read(authProvider).value?.status,
      AuthStatus.unauthenticated,
    );
    expect(
      h.server.requests.where((r) => r.url.path == '/auth/logout'),
      hasLength(1),
    );
    expect(h.server.requests.where((r) => r.method == 'DELETE'), isEmpty);
  });

  group('Account (token)', () {
    testWidgets('shows the account, not the nsec, even with a legacy key', (
      tester,
    ) async {
      final h = TokenSettingsHarness(nsec: nostr.Keys.generate().nsec);
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await h.pump(tester, _settings());

      expect(find.text('Identity (pubkey)'), findsNothing);
      await tester.scrollUntilVisible(
        find.byKey(const Key('settings-account-row')),
        200,
      );
      expect(find.text('Ada'), findsOneWidget);
      expect(find.textContaining('@ada'), findsOneWidget);
      expect(find.byKey(const Key('settings-devices-row')), findsOneWidget);

      // Keys are never shown: there is no Account ID row.
      expect(find.byKey(const Key('settings-account-id-row')), findsNothing);
    });

    testWidgets('the account rows open Account and Devices', (tester) async {
      final h = TokenSettingsHarness();
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await h.pump(tester, _settings());

      await _centerInView(
        tester,
        find.byKey(const Key('settings-account-row')),
      );
      await tester.tap(find.byKey(const Key('settings-account-row')));
      await frames(tester);
      expect(find.byType(AccountPage), findsOneWidget);
      await tester.pageBack();
      await tester.pumpAndSettle();

      await _centerInView(
        tester,
        find.byKey(const Key('settings-devices-row')),
      );
      await tester.tap(find.byKey(const Key('settings-devices-row')));
      await frames(tester);
      expect(find.byType(DevicesPage), findsOneWidget);
    });

    testWidgets('Edit opens the global account editor', (tester) async {
      final h = TokenSettingsHarness();
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await h.pump(tester, _profile());

      await tester.tap(find.byKey(const ValueKey('settings-edit-profile')));
      await frames(tester);

      expect(find.byType(AccountPage), findsOneWidget);
      expect(
        find.byKey(const ValueKey('edit-profile-sheet-content')),
        findsNothing,
      );
    });
  });
}
