import 'package:buzz/features/settings/settings_page.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/default_community.dart';
import 'package:buzz/shared/community/community_membership_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:buzz/shared/utils/string_utils.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:package_info_plus/package_info_plus.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../helpers/widget_helpers.dart';

void main() {
  testWidgets('account card hides the signing key and offers Google', (
    tester,
  ) async {
    PackageInfo.setMockInitialValues(
      appName: 'Buzz',
      packageName: 'xyz.block.buzz',
      version: 'test',
      buildNumber: '1',
      buildSignature: '',
    );
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();

    await tester.pumpWidget(
      WidgetHelpers.testable(
        overrides: [
          relayConfigProvider.overrideWith(_RelayConfigNotifier.new),
          relaySessionProvider.overrideWith(_DisconnectedSession.new),
          authProvider.overrideWith(_AuthNotifier.new),
          savedPrefsProvider.overrideWithValue(prefs),
          currentCommunityRoleProvider.overrideWithValue(
            const AsyncData<CommunityMemberRole?>(CommunityMemberRole.member),
          ),
        ],
        child: SettingsPage(
          profileHeader: const SizedBox.shrink(),
          invitePageBuilder: (_) => const SizedBox.shrink(),
        ),
      ),
    );
    await tester.pump();
    final google = find.byKey(const Key('settings-google-key-backup'));
    await tester.scrollUntilVisible(google, 200);
    await tester.pumpAndSettle();

    final npub = fullNpub(nostr.Keys(_RelayConfigNotifier.privHex).public);
    expect(find.text('Account'), findsOneWidget);
    expect(find.text('Connection'), findsNothing);
    expect(
      find.descendant(
        of: google,
        matching: find.text('Connect Google account'),
      ),
      findsOneWidget,
    );
    expect(find.text('Use Google to sign in on other devices'), findsOneWidget);
    // The signing key is never surfaced in any form: no row, copy action,
    // or assistive value.
    expect(find.text('Identity (pubkey)'), findsNothing);
    expect(find.byIcon(LucideIcons.copy), findsNothing);
    expect(find.textContaining('pubkey', findRichText: true), findsNothing);
    expect(
      find.byWidgetPredicate(
        (widget) => widget is Semantics && widget.properties.value == npub,
      ),
      findsNothing,
    );

    expect(
      find.byKey(const Key('settings-remove-community')).hitTestable(),
      findsNothing,
    );
    await tester.scrollUntilVisible(find.text('Remove community'), 200);
    await tester.tap(find.text('Remove community'));
    await tester.pumpAndSettle();
    await tester.ensureVisible(
      find.byKey(const Key('settings-remove-community')),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('settings-remove-community')));
    await tester.pumpAndSettle();
    expect(find.text('Remove community from this device?'), findsOneWidget);
    expect(
      find.textContaining('won’t be able to sign back in'),
      findsOneWidget,
    );
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('with the built-in server settings have no community controls', (
    tester,
  ) async {
    PackageInfo.setMockInitialValues(
      appName: 'Buzz',
      packageName: 'xyz.block.buzz',
      version: 'test',
      buildNumber: '1',
      buildSignature: '',
    );
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();

    await tester.pumpWidget(
      WidgetHelpers.testable(
        overrides: [
          defaultCommunityOriginProvider.overrideWithValue(
            'https://relay.test',
          ),
          relayConfigProvider.overrideWith(_RelayConfigNotifier.new),
          relaySessionProvider.overrideWith(_DisconnectedSession.new),
          authProvider.overrideWith(_AuthNotifier.new),
          savedPrefsProvider.overrideWithValue(prefs),
          currentCommunityRoleProvider.overrideWithValue(
            const AsyncData<CommunityMemberRole?>(CommunityMemberRole.admin),
          ),
        ],
        child: SettingsPage(
          profileHeader: const SizedBox.shrink(),
          invitePageBuilder: (_) => const SizedBox.shrink(),
        ),
      ),
    );
    await tester.pump();

    expect(find.text('Invite people'), findsOneWidget);
    expect(find.text('Invite to community'), findsNothing);
    expect(find.text('Community'), findsNothing);

    final signOut = find.byKey(const Key('settings-sign-out'));
    await tester.scrollUntilVisible(signOut, 200);
    await tester.pumpAndSettle();
    expect(find.text('Remove community'), findsNothing);
    expect(find.byKey(const Key('settings-removal-management')), findsNothing);
    expect(find.textContaining('communities'), findsNothing);

    await tester.tap(signOut);
    await tester.pumpAndSettle();
    expect(find.text('Sign out?'), findsOneWidget);
    expect(find.textContaining('communit'), findsNothing);
    await tester.tap(find.byKey(const Key('settings-sign-out-cancel')));
    await tester.pumpAndSettle();
  });
}

class _AuthNotifier extends AuthNotifier {
  @override
  Future<AuthState> build() async => AuthState(
    status: AuthStatus.authenticated,
    community: Community(
      id: 'community',
      name: 'Test',
      relayUrl: 'https://relay.test',
      nsec: _RelayConfigNotifier.nsec,
      addedAt: DateTime.utc(2026),
    ),
  );
}

class _DisconnectedSession extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
}

class _RelayConfigNotifier extends RelayConfigNotifier {
  static const privHex =
      '1111111111111111111111111111111111111111111111111111111111111111';
  static final nsec = nostr.Keys(privHex).nsec;

  @override
  RelayConfig build() => RelayConfig(baseUrl: 'https://relay.test', nsec: nsec);
}
