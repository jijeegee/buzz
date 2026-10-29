import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:path_provider/path_provider.dart';
import 'package:sqflite/sqflite.dart';

import 'message_store.dart';

/// The app's message store, under the support directory (kept until the
/// account signs out, unlike the cache directory).
final messageStoreProvider = Provider<MessageStore>(
  (ref) => MessageStore(
    factory: () => databaseFactory,
    directory: () async =>
        '${(await getApplicationSupportDirectory()).path}/messages',
  ),
);
