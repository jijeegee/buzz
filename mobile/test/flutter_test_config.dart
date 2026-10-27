import 'dart:async';

import 'package:kuzz/shared/auth/default_community.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_capture.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

/// Runs before every test file. The app always pins the built-in server
/// (`kuzzServerOrigin`); suites that use fake relay hosts run with the
/// fixed-server rule off. Tests for the rule override
/// `defaultCommunityOriginProvider` explicitly.
Future<void> testExecutable(FutureOr<void> Function() testMain) async {
  debugUnrestrictedServerForTests = true;
  sqfliteFfiInit();
  messageStoreFactoryForTests = () => MessageStore(
    () async => inMemoryDatabasePath,
    factory: databaseFactoryFfiNoIsolate,
  );
  await testMain();
}
