import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:path_provider/path_provider.dart';
import 'package:sqlite3/sqlite3.dart';

import '../relay/relay.dart';
import 'message_db.dart';
import 'message_db_writer.dart';

/// Whose messages a database file holds: one account on one server.
@immutable
class MessageScope {
  const MessageScope({required this.relayUrl, required this.pubkey});

  /// The server's physical address. Files are keyed by its storage scope
  /// (so a service address change keeps them); sync state by the address.
  final String relayUrl;
  final String pubkey;

  /// The file's path below the messages directory.
  String get relativePath =>
      '${_hash(storageScopeFor(relayUrl))}/${_hash(pubkey)}.db';

  @override
  bool operator ==(Object other) =>
      other is MessageScope &&
      other.relayUrl == relayUrl &&
      other.pubkey == pubkey;

  @override
  int get hashCode => Object.hash(relayUrl, pubkey);
}

/// The scope of a stored account whose server is [relayUrl] (as persisted),
/// or null when its identity is unknown.
MessageScope? messageScopeOf({
  required String relayUrl,
  String? pubkey,
  String? nsec,
}) {
  final key = pubkey ?? pubkeyFromNsec(nsec);
  if (key == null) return null;
  return MessageScope(
    relayUrl: RelayConfig(baseUrl: relayUrl).baseUrl,
    pubkey: key,
  );
}

String _hash(String value) => sha256.convert(utf8.encode(value)).toString();

/// The message databases on this device, one writer per open file.
///
/// Opening a file and clearing one run one at a time, so a clear never
/// deletes a file a writer still has open, and a write queued before a clear
/// is dropped rather than recreating the cleared file.
class MessageStore {
  /// [rootPath], when known up front, lets [reader] open files before any
  /// write resolved [_root].
  MessageStore(this._root, {this.onDirectoryCreated, String? rootPath})
    : _knownRoot = rootPath;

  final Future<String> Function() _root;
  String? _knownRoot;

  /// Called once with the messages directory after it is created (iOS
  /// excludes it from backup there).
  final Future<void> Function(String path)? onDirectoryCreated;

  Future<String?>? _rootPath;
  final _writers = <String, MessageDbWriter>{};

  /// Read-only connections on this isolate, by file.
  final _readers = <String, Database>{};

  /// Files with a clear in progress (and every file while a clear of all
  /// runs): [reader] opens none of them meanwhile.
  final _clearing = <String, int>{};
  int _clearingAll = 0;
  Future<void> _serial = Future.value();
  int _generation = 0;
  int _clearedAll = 0;
  final _clearedAt = <String, int>{};
  bool _closed = false;

  /// The current write generation; pass it to [write] to drop the write when
  /// a clear of its scope happens in between.
  int get generation => _generation;

  /// Whether a write queued under [generation] belongs to data of [scope]
  /// cleared since.
  bool isCleared(MessageScope scope, int generation) =>
      _isStale(scope.relativePath, generation);

  bool _isStale(String key, int generation) =>
      generation < _clearedAll || generation < (_clearedAt[key] ?? 0);

  Future<T> _serialized<T>(Future<T> Function() task) {
    final result = _serial.then((_) => task());
    _serial = result.then<void>((_) {}, onError: (_) {});
    return result;
  }

  Future<String?> _resolveRoot() => _rootPath ??= () async {
    try {
      final path = await _root();
      final directory = Directory(path);
      if (!directory.existsSync()) {
        directory.createSync(recursive: true);
      }
      try {
        await onDirectoryCreated?.call(path);
      } catch (error) {
        debugPrint('[MessageStore] directory setup failed: $error');
      }
      return _knownRoot = path;
    } catch (error) {
      // No storage on this platform (or in this test): writes are dropped.
      debugPrint('[MessageStore] messages directory unavailable: $error');
      return null;
    }
  }();

  /// Applies [write] to [scope]'s database in one transaction. Dropped when
  /// [scope] was cleared after [generation] (default: now) or storage is
  /// unavailable.
  Future<void> write(
    MessageScope scope,
    MessageDbWrite write, {
    int? generation,
  }) async {
    if (write.isEmpty) return;
    final key = scope.relativePath;
    final asOf = generation ?? _generation;
    // Sent to the writer inside the serialized step, so a later clear or
    // close always reaches the writer after it. The transaction itself is
    // awaited outside, so writes to different files do not wait on each
    // other.
    final sent = await _serialized(() async {
      if (_closed || _isStale(key, asOf)) return null;
      final root = await _resolveRoot();
      if (root == null) return null;
      final writer = _writers[key] ??= MessageDbWriter.start('$root/$key');
      return (writer.write(write),);
    });
    if (sent != null) await sent.$1;
  }

  /// Runs [query] on [scope]'s database after every write requested so far
  /// (see [MessageDbWriter.read]). Null when [scope] has no database yet or
  /// storage is unavailable; a read never creates the file.
  Future<R?> read<R>(MessageScope scope, MessageDbRead<R> query) async {
    final key = scope.relativePath;
    final sent = await _serialized(() async {
      if (_closed) return null;
      final root = await _resolveRoot();
      if (root == null) return null;
      var writer = _writers[key];
      if (writer == null) {
        if (!File('$root/$key').existsSync()) return null;
        writer = _writers[key] = MessageDbWriter.start('$root/$key');
      }
      return (writer.read(query),);
    });
    return sent == null ? null : await sent.$1;
  }

  /// [scope]'s database opened for reads on this isolate, or null when it
  /// does not exist yet (or storage is not known yet). Synchronous: a room
  /// paints its first frame from it. The connection stays open until the
  /// file is cleared or the store closes.
  Database? reader(MessageScope scope) {
    final key = scope.relativePath;
    final open = _readers[key];
    if (open != null) return open;
    final root = _knownRoot;
    if (_closed ||
        root == null ||
        _clearingAll > 0 ||
        (_clearing[key] ?? 0) > 0) {
      return null;
    }
    final path = '$root/$key';
    if (!File(path).existsSync()) return null;
    try {
      return _readers[key] = openMessageDbReader(path);
    } on SqliteException catch (error) {
      debugPrint('[MessageStore] cannot read $key: $error');
      return null;
    }
  }

  void _closeReader(String key) => _readers.remove(key)?.close();

  void _closeReaders() {
    for (final reader in _readers.values) {
      reader.close();
    }
    _readers.clear();
  }

  /// Whether [scope]'s database can exist on this device: false when
  /// storage is unavailable.
  Future<bool> get isAvailable async => await _resolveRoot() != null;

  /// Resolves once every write requested so far has been applied (or has
  /// failed).
  Future<void> settled() async {
    final writers = await _serialized(() async => _writers.values.toList());
    await Future.wait(writers.map((writer) => writer.settled()));
  }

  /// Deletes [scope]'s database (signing out or removing that account). The
  /// account's other servers and other accounts keep theirs.
  Future<void> clear(MessageScope scope) {
    final key = scope.relativePath;
    _clearedAt[key] = ++_generation;
    _clearing[key] = (_clearing[key] ?? 0) + 1;
    _closeReader(key);
    return _serialized(() async {
      try {
        await _writers.remove(key)?.close();
        final root = await _resolveRoot();
        if (root == null) return;
        _deleteDatabase('$root/$key');
      } finally {
        final left = _clearing[key]! - 1;
        if (left == 0) {
          _clearing.remove(key);
        } else {
          _clearing[key] = left;
        }
      }
    });
  }

  /// Deletes every database on this device.
  Future<void> clearAll() {
    _clearedAll = ++_generation;
    _clearingAll++;
    _closeReaders();
    return _serialized(() async {
      try {
        await _closeWriters();
        final root = await _resolveRoot();
        if (root == null) return;
        final directory = Directory(root);
        if (directory.existsSync()) {
          for (final entry in directory.listSync()) {
            entry.deleteSync(recursive: true);
          }
        }
      } finally {
        _clearingAll--;
      }
    });
  }

  Future<void> close() {
    _closed = true;
    _closeReaders();
    return _serialized(_closeWriters);
  }

  Future<void> _closeWriters() async {
    final writers = _writers.values.toList();
    _writers.clear();
    await Future.wait(writers.map((writer) => writer.close()));
  }
}

void _deleteDatabase(String path) {
  for (final suffix in const ['', '-wal', '-shm', '-journal']) {
    final file = File('$path$suffix');
    if (file.existsSync()) file.deleteSync();
  }
}

const _backupChannel = MethodChannel('kuzz/backup_exclusion');

/// Keeps the messages directory out of iCloud and device backups on iOS.
/// Android excludes it in its backup rules (res/xml).
Future<void> _excludeFromBackup(String path) async {
  if (!Platform.isIOS) return;
  await _backupChannel.invokeMethod<void>('exclude', path);
}

/// The messages directory, resolved in `main` before the first frame so a
/// room can read its stored rows synchronously. Null (tests, other entry
/// points) resolves it on first use instead, so nothing is read before then.
final messagesRootProvider = Provider<String?>((ref) => null);

/// The messages directory below the app's support directory (never the
/// cache directory, which the OS may empty).
Future<String> resolveMessagesRoot() async =>
    '${(await getApplicationSupportDirectory()).path}/messages';

/// Deletes the channel heads earlier versions kept in the cache directory
/// (every page they held was also written to the message database).
Future<void> deleteLegacyChannelHeads() async {
  try {
    final heads = Directory(
      '${(await getApplicationCacheDirectory()).path}/channel_heads',
    );
    if (await heads.exists()) await heads.delete(recursive: true);
  } catch (error) {
    debugPrint('[MessageStore] old channel heads not removed: $error');
  }
}

final messageStoreProvider = Provider<MessageStore>((ref) {
  final rootPath = ref.watch(messagesRootProvider);
  final store = MessageStore(
    () async => rootPath ?? await resolveMessagesRoot(),
    onDirectoryCreated: _excludeFromBackup,
    rootPath: rootPath,
  );
  ref.onDispose(() => unawaited(store.close()));
  return store;
});

/// The active account's message scope, or null without an identity.
final messageScopeProvider = Provider<MessageScope?>((ref) {
  final pubkey = ref.watch(myPubkeyProvider);
  if (pubkey == null) return null;
  return MessageScope(
    relayUrl: ref.watch(relayConfigProvider).baseUrl,
    pubkey: pubkey,
  );
});
