import 'package:buzz/features/settings/account_page.dart';
import 'package:buzz/features/settings/devices_page.dart';
import 'package:buzz/features/settings/settings_page.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/utils/string_utils.dart';
import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../shared/auth/token/token_auth_test_fakes.dart';
import 'token_settings_harness.dart';

Widget _settings() => SettingsPage(
  profileHeader: const SizedBox.shrink(),
  invitePageBuilder: (_) => const SizedBox.shrink(),
);

Future<void> _tapRemoveAndConfirm(WidgetTester tester) async {
  await tester.scrollUntilVisible(find.text('Sign out'), 200);
  await tester.tap(find.text('Sign out'));
  await frames(tester);
  await tester.tap(find.widgetWithText(FilledButton, 'Sign out'));
  await frames(tester);
}

void main() {
  group('Sign out (token)', () {
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

      final copied = <String>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied.add((call.arguments as Map)['text'] as String);
          }
          return null;
        },
      );
      await tester.tap(find.byKey(const Key('settings-account-id-row')));
      await frames(tester);
      expect(copied, [fullNpub(tokenPrincipal)]);

      // The merged button node owns the tap: screen-reader activation copies
      // too (AGENTS.md rule 7).
      final semantics = tester.ensureSemantics();
      await tester.pump();
      final node = tester.getSemantics(
        find.byKey(const Key('settings-account-id-row')),
      );
      expect(node.label, 'Copy account public key');
      expect(node.getSemanticsData().hasAction(SemanticsAction.tap), isTrue);
      node.owner!.performAction(node.id, SemanticsAction.tap);
      await frames(tester);
      expect(copied, [fullNpub(tokenPrincipal), fullNpub(tokenPrincipal)]);
      semantics.dispose();
    });

    testWidgets('the account rows open Account and Devices', (tester) async {
      final h = TokenSettingsHarness();
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await h.pump(tester, _settings());

      await tester.scrollUntilVisible(
        find.byKey(const Key('settings-account-row')),
        200,
      );
      await tester.tap(find.byKey(const Key('settings-account-row')));
      await frames(tester);
      expect(find.byType(AccountPage), findsOneWidget);
      await tester.pageBack();
      await tester.pumpAndSettle();

      await tester.tap(find.byKey(const Key('settings-devices-row')));
      await frames(tester);
      expect(find.byType(DevicesPage), findsOneWidget);
    });

    testWidgets('Edit opens the global account editor', (tester) async {
      final h = TokenSettingsHarness();
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await h.pump(tester, _settings());

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
