import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/sign_in/device_key_notice.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/auth/token/token.dart';

import '../../helpers/widget_helpers.dart';

const _origin = 'https://relay.test';

void main() {
  Future<void> pump(WidgetTester tester, {required bool? deviceKey}) async {
    await tester.pumpWidget(
      WidgetHelpers.testable(
        overrides: [
          accountOriginProvider.overrideWithValue(_origin),
          activeGoogleKeyOriginProvider.overrideWithValue(null),
          accountProfileProvider(_origin).overrideWith(
            (ref) async => AccountProfile(
              principalId: 'p',
              displayName: 'P',
              deviceKey: deviceKey,
            ),
          ),
        ],
        child: const DeviceKeyNotice(child: Text('home')),
      ),
    );
    await tester.pump();
  }

  testWidgets('a keyless session shows the sign-in-again banner', (
    tester,
  ) async {
    await pump(tester, deviceKey: false);
    expect(find.text('Sign in again to finish a security update'), findsOne);
    expect(find.byKey(const Key('device-key-sign-in-again')), findsOne);
    expect(find.text('home'), findsOne);
  });

  for (final deviceKey in [true, null]) {
    testWidgets('device_key $deviceKey shows no banner', (tester) async {
      await pump(tester, deviceKey: deviceKey);
      expect(find.byKey(const Key('device-key-banner')), findsNothing);
      expect(find.text('home'), findsOne);
    });
  }
}
