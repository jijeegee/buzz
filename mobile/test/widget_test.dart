import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:buzz/app.dart';
import 'package:buzz/features/age_gate/age_signal_provider.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/theme/theme_provider.dart';

void main() {
  testWidgets('App renders token sign-in onboarding when unauthenticated', (
    WidgetTester tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          authProvider.overrideWith(() => _FakeAuthNotifier()),
          ageSignalProvider.overrideWith(() => _AllowedAgeSignalNotifier()),
          savedPrefsProvider.overrideWithValue(prefs),
        ],
        child: const App(),
      ),
    );
    await tester.pump();
    expect(find.byKey(const Key('token-sign-in-relay-url')), findsOneWidget);
    expect(find.text('Sign in to a relay'), findsOneWidget);
    // NIP-AB pairing is gone: no QR scanner entry point.
    expect(find.text('Scan a QR code'), findsNothing);
  });

  test('NIP-AB pairing feature is removed from the app', () {
    expect(Directory('lib/features/pairing').existsSync(), isFalse);
    final offenders = Directory('lib')
        .listSync(recursive: true)
        .whereType<File>()
        .where((file) => file.path.endsWith('.dart'))
        .where((file) {
          final source = file.readAsStringSync();
          return source.contains('features/pairing/') ||
              source.contains('pairing_provider.dart') ||
              source.contains('PairingPage');
        })
        .map((file) => file.path)
        .toList();
    expect(offenders, isEmpty);
  });
}

class _AllowedAgeSignalNotifier extends AgeSignalNotifier {
  @override
  AgeSignalState build() => AgeSignalState.allowed;

  @override
  Future<void> request() async {}
}

class _FakeAuthNotifier extends AuthNotifier {
  @override
  Future<AuthState> build() async {
    return const AuthState(status: AuthStatus.unauthenticated);
  }
}
