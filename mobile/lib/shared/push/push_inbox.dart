import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../community/community.dart';
import '../community/community_provider.dart';
import '../relay/app_lifecycle_provider.dart';
import '../relay/relay_session.dart';
import 'push_snapshot.dart';

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
  if (!pushInboxEnabled || defaultTargetPlatform != TargetPlatform.iOS) {
    return null;
  }
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
