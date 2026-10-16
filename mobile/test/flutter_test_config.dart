import 'dart:async';

import 'package:buzz/shared/auth/default_community.dart';

/// Runs before every test file. The app always pins the built-in server
/// (`kuzzServerOrigin`); suites that use fake relay hosts run with the
/// fixed-server rule off. Tests for the rule override
/// `defaultCommunityOriginProvider` explicitly.
Future<void> testExecutable(FutureOr<void> Function() testMain) async {
  debugUnrestrictedServerForTests = true;
  await testMain();
}
