import 'package:buzz/features/settings/settings_page.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/community/community_membership_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:package_info_plus/package_info_plus.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../helpers/widget_helpers.dart';

void main() {
  testWidgets('shows a compact copyable identity row', (tester) async {
    PackageInfo.setMockInitialValues(
      appName: 'Buzz',
      packageName: 'xyz.block.buzz',
      version: 'test',
      buildNumber: '1',
      buildSignature: '',
    );
    MethodCall? clipboardCall;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'Clipboard.setData') clipboardCall = call;
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
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
    await tester.ensureVisible(find.text('Identity (pubkey)'));
    await tester.pumpAndSettle();

    final expectedPubkey = nostr.Keys(
      '1111111111111111111111111111111111111111111111111111111111111111',
    ).public;
    // Keys('1'×64).public ↔ this npub — the NIP-19 canonical vector for the
    // identity row, hardcoded so the codec itself stays under test.
    const expectedNpub =
        'npub1fu64hh9hes90w2808n8tjc2ajp5yhddjef0ctx4s7zmsgp6cwx4qgy4eg9';
    expect(find.text('Connected to'), findsNothing);
    expect(find.text('https://relay.test'), findsNothing);
    expect(find.byKey(const Key('settings-google-key-backup')), findsOneWidget);
    // Neither the raw hex key nor the full npub is rendered visually — the
    // full npub is exposed through a11y and the clipboard only.
    expect(find.text(expectedPubkey), findsNothing);
    expect(find.text(expectedNpub), findsNothing);
    // The identity row's a11y value carries the full npub (not raw hex).
    expect(
      find.ancestor(
        of: find.text('Identity (pubkey)'),
        matching: find.byWidgetPredicate(
          (widget) =>
              widget is Semantics && widget.properties.value == expectedNpub,
        ),
      ),
      findsOneWidget,
    );
    final copy = tester.getRect(find.byIcon(LucideIcons.copy));
    final chevron = tester.getRect(find.byIcon(LucideIcons.chevronRight).first);
    expect(copy.center.dx, closeTo(chevron.center.dx, 0.5));

    await tester.tap(find.text('Identity (pubkey)'));
    await tester.pump();
    expect(clipboardCall?.method, 'Clipboard.setData');
    expect(clipboardCall?.arguments, {'text': expectedNpub});
    expect(find.text('Pubkey copied'), findsOneWidget);
    await tester.scrollUntilVisible(find.text('Sign out'), 200);
    await tester.tap(find.text('Sign out'));
    await tester.pumpAndSettle();
    expect(find.textContaining('The saved private key'), findsOneWidget);
    expect(find.textContaining('Before continuing'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
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
  static final nsec = nostr.Keys(
    '1111111111111111111111111111111111111111111111111111111111111111',
  ).nsec;

  @override
  RelayConfig build() => RelayConfig(baseUrl: 'https://relay.test', nsec: nsec);
}
