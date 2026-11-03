import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../community/community.dart';
import '../community/community_provider.dart';
import '../message_store/message_store.dart' show messageStoreOrigin;
import '../relay/app_lifecycle_provider.dart';
import '../relay/relay_session.dart';
import 'android_push.dart' show androidPushActive, readAndroidPushInboxRoot;
import 'push_snapshot.dart';
import 'push_subscription.dart' show kuzzPushEligibleKinds;

/// The push inbox (docs/message-sync.md#push-inbox): events the iPhone
/// notification extension fetched, kept per community token under
/// `<App Group>/push-inbox/<token>/` for the app to take into its store.

const _channel = MethodChannel('kuzz/push');
final _tokenPattern = RegExp(r'^[0-9a-f]{32}$');

/// Whether [token] can name an inbox folder (32 lowercase hex).
bool isPushInboxToken(String? token) =>
    token != null && _tokenPattern.hasMatch(token);

typedef PushInboxRootReader = Future<String?> Function();

/// The `push-inbox` folder path, or null off iOS, with the inbox off, or
/// when the App Group is unavailable.
final pushInboxRootProvider = Provider<PushInboxRootReader>(
  (ref) => readPushInboxRoot,
);

Future<String?> readPushInboxRoot() async {
  if (!pushInboxEnabled) return null;
  // Android push builds: the wake engine's inbox in no-backup files.
  if (androidPushActive) return readAndroidPushInboxRoot();
  if (defaultTargetPlatform != TargetPlatform.iOS) return null;
  try {
    return await _channel.invokeMethod<String>('pushInboxRoot');
  } on MissingPluginException {
    return null;
  } on PlatformException catch (error) {
    debugPrint('[PushInbox] root unavailable: ${error.code}');
    return null;
  }
}

/// Replaces `<root>/<token>/cursor` with [seconds], atomically. The NSE
/// then fetches only from there (minus its lookback).
Future<void> writePushInboxCursor(
  String root,
  String token,
  int seconds,
) async {
  if (!isPushInboxToken(token)) return;
  final folder = Directory('$root/$token');
  await folder.create(recursive: true);
  final suffix = Random().nextInt(1 << 32).toRadixString(16);
  final temporary = File('${folder.path}/cursor.$suffix.tmp');
  await temporary.writeAsString('$seconds', flush: true);
  await temporary.rename('${folder.path}/cursor');
}

/// On pause while the socket is connected, the store already holds what
/// the NSE would fetch before now, so the active community's cursor moves
/// to now. This only narrows what the NSE fetches; nothing reads it as
/// coverage.
Future<void> notePushInboxPause({
  required String? root,
  required Community? community,
  required bool connected,
  required DateTime now,
}) async {
  final token = community?.pushInboxToken;
  if (root == null ||
      !connected ||
      community == null ||
      community.signedOut ||
      !community.pushNotificationsEnabled ||
      !isPushInboxToken(token)) {
    return;
  }
  await writePushInboxCursor(root, token!, now.millisecondsSinceEpoch ~/ 1000);
}

/// Listens for the app going to the background; see [notePushInboxPause].
final pushInboxPauseCursorProvider = Provider<void>((ref) {
  if (!pushInboxEnabled) return;
  ref.listen(appLifecycleProvider, (previous, next) {
    if (next != AppLifecycleState.hidden && next != AppLifecycleState.paused) {
      return;
    }
    final connected =
        ref.read(relaySessionProvider).status == SessionStatus.connected;
    final community = ref.read(activeCommunityProvider).value;
    final readRoot = ref.read(pushInboxRootProvider);
    unawaited(() async {
      try {
        await notePushInboxPause(
          root: await readRoot(),
          community: community,
          connected: connected,
          now: DateTime.now(),
        );
      } catch (error) {
        debugPrint('[PushInbox] cursor not written: $error');
      }
    }());
  });
});

/// Deletes the inbox folders of [tokens] (sign-out, removal). Errors are
/// logged: the launch sweep removes what is left.
Future<void> deletePushInboxFolders(
  PushInboxRootReader readRoot,
  Iterable<String?> tokens,
) async {
  final valid = tokens.where(isPushInboxToken).toList();
  if (valid.isEmpty) return;
  try {
    final root = await readRoot();
    if (root == null) return;
    for (final token in valid) {
      final folder = Directory('$root/$token');
      if (await folder.exists()) await folder.delete(recursive: true);
    }
  } catch (error) {
    debugPrint('[PushInbox] folder not deleted: $error');
  }
}

/// Deletes token folders under [root] that no community holds now.
Future<void> sweepPushInbox(String root, Set<String> liveTokens) async {
  try {
    final dir = Directory(root);
    if (!await dir.exists()) return;
    await for (final entry in dir.list()) {
      final name = entry.uri.pathSegments.lastWhere((s) => s.isNotEmpty);
      if (entry is Directory &&
          isPushInboxToken(name) &&
          !liveTokens.contains(name)) {
        await entry.delete(recursive: true);
      }
    }
  } catch (error) {
    debugPrint('[PushInbox] sweep failed: $error');
  }
}

/// A file larger than the NSE's whole per-token cap is refused unread.
const pushInboxMaxFileBytes = 2097152;

/// The store origin of an inbox file's `relayUrl`: as [messageStoreOrigin],
/// but a path, user info, query or fragment is refused (null).
String? pushInboxOrigin(String relayUrl) {
  final uri = Uri.tryParse(relayUrl.trim());
  if (uri == null ||
      uri.userInfo.isNotEmpty ||
      (uri.path.isNotEmpty && uri.path != '/') ||
      uri.hasQuery ||
      uri.hasFragment) {
    return null;
  }
  return messageStoreOrigin(relayUrl);
}

/// One accepted inbox file: its push-kind events (ascending) for the store
/// and its kind:0 profiles.
@immutable
class PushInboxFileRead {
  const PushInboxFileRead({
    required this.path,
    required this.fetchedAt,
    required this.events,
    required this.profiles,
  });

  final String path;
  final int fetchedAt;
  final List<Map<String, dynamic>> events;
  final List<Map<String, dynamic>> profiles;
}

/// What a folder read found: files to take and files to delete unread.
/// Files of a newer format are in neither and stay.
@immutable
class PushInboxRead {
  const PushInboxRead(this.files, this.refused);

  static const empty = PushInboxRead([], []);

  final List<PushInboxFileRead> files;
  final List<String> refused;
}

/// Reads `<root>/<token>` off the UI isolate; see [readPushInboxFolderSync].
Future<PushInboxRead> readPushInboxFolder({
  required String folder,
  required String relayUrl,
  required String account,
}) => Isolate.run(
  () => readPushInboxFolderSync(
    folder: folder,
    relayUrl: relayUrl,
    account: account,
  ),
);

/// The inbox files in [folder] for [account] on [relayUrl]'s server, oldest
/// first (docs/message-sync.md#push-inbox). `.tmp` files and the cursor
/// are ignored; a missing folder or file is no error. A file for another
/// server or account, malformed, or oversize is refused. Events of the
/// wrong shape or kind are left out; the NSE verified their signatures.
PushInboxRead readPushInboxFolderSync({
  required String folder,
  required String relayUrl,
  required String account,
}) {
  final origin = messageStoreOrigin(relayUrl);
  final dir = Directory(folder);
  final List<File> entries;
  try {
    if (origin == null || !dir.existsSync()) return PushInboxRead.empty;
    entries =
        dir
            .listSync()
            .whereType<File>()
            .where((file) => file.path.endsWith('.json'))
            .toList()
          ..sort((a, b) => a.path.compareTo(b.path));
  } on FileSystemException {
    return PushInboxRead.empty;
  }
  final files = <PushInboxFileRead>[];
  final refused = <String>[];
  for (final file in entries) {
    final Object? decoded;
    try {
      if (file.lengthSync() > pushInboxMaxFileBytes) {
        refused.add(file.path);
        continue;
      }
      decoded = jsonDecode(file.readAsStringSync());
    } on FileSystemException {
      continue;
    } on FormatException {
      refused.add(file.path);
      continue;
    }
    if (decoded is Map && decoded['v'] is int && (decoded['v'] as int) > 1) {
      continue;
    }
    final fileRelay = decoded is Map ? decoded['relayUrl'] : null;
    final fileAccount = decoded is Map ? decoded['account'] : null;
    final fetchedAt = decoded is Map ? decoded['fetchedAt'] : null;
    final events = decoded is Map ? decoded['events'] : null;
    if (decoded is! Map ||
        decoded['v'] != 1 ||
        fileRelay is! String ||
        pushInboxOrigin(fileRelay) != origin ||
        fileAccount is! String ||
        fileAccount.toLowerCase() != account.toLowerCase() ||
        fetchedAt is! int ||
        events is! List) {
      refused.add(file.path);
      continue;
    }
    final seen = <String>{};
    final kept = <Map<String, dynamic>>[];
    final profiles = <Map<String, dynamic>>[];
    for (final event in events) {
      if (!_wellFormed(event) || !seen.add(event['id'] as String)) continue;
      final map = Map<String, dynamic>.from(event as Map);
      (map['kind'] == 0 ? profiles : kept).add(map);
    }
    kept.sort(
      (a, b) => a['created_at'] != b['created_at']
          ? (a['created_at'] as int).compareTo(b['created_at'] as int)
          : (a['id'] as String).compareTo(b['id'] as String),
    );
    files.add(
      PushInboxFileRead(
        path: file.path,
        fetchedAt: fetchedAt,
        events: kept,
        profiles: profiles,
      ),
    );
  }
  return PushInboxRead(files, refused);
}

final _hex64 = RegExp(r'^[0-9a-f]{64}$');
final _hex128 = RegExp(r'^[0-9a-f]{128}$');

bool _wellFormed(Object? event) {
  if (event is! Map) return false;
  final id = event['id'];
  final pubkey = event['pubkey'];
  final sig = event['sig'];
  final kind = event['kind'];
  final tags = event['tags'];
  return id is String &&
      _hex64.hasMatch(id) &&
      pubkey is String &&
      _hex64.hasMatch(pubkey) &&
      sig is String &&
      _hex128.hasMatch(sig) &&
      event['created_at'] is int &&
      kind is int &&
      (kind == 0 || kuzzPushEligibleKinds.contains(kind)) &&
      event['content'] is String &&
      tags is List &&
      tags.every((tag) => tag is List && tag.every((part) => part is String));
}
