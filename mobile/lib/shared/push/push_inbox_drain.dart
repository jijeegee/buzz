import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../features/age_gate/age_signal_provider.dart';
import '../community/community.dart';
import '../community/community_provider.dart';
import '../message_store/message_store.dart';
import '../message_store/message_store_provider.dart';
import '../profile/user_cache_provider.dart';
import '../relay/app_lifecycle_provider.dart';
import '../relay/nostr_models.dart';
import '../relay/relay_provider.dart';
import 'push_inbox.dart';
import 'push_snapshot.dart';

/// Takes the push inbox (docs/message-sync.md#push-inbox) into the message
/// store: on launch, on an account or community change and on resume.

/// Folds the events the store accepted into a list. Gets the lease the
/// drain ran under and checks it is still current.
typedef PushInboxFold =
    void Function(MessageStoreLease lease, List<NostrEvent> events);

/// The list folds (docs/message-sync.md#push-inbox).
final pushInboxFoldsProvider = Provider<List<PushInboxFold>>((ref) => const []);

/// What one drain works on.
typedef PushInboxDrainScope = ({
  MessageStoreLease lease,
  String token,
  String relayUrl,
});

/// The scope to drain now, or null: age-restricted, no signed-in community
/// with an inbox token, or no store lease.
PushInboxDrainScope? pushInboxDrainScope({
  required bool ageRestricted,
  required Community? community,
  required String relayUrl,
  required String? account,
  required MessageStore store,
}) {
  final token = community?.pushInboxToken;
  if (ageRestricted ||
      community == null ||
      community.signedOut ||
      !isPushInboxToken(token)) {
    return null;
  }
  final lease = store.lease(relayUrl, account);
  if (lease == null) return null;
  return (lease: lease, token: token!, relayUrl: relayUrl);
}

/// One drain of [scope]'s folder under [root]: refused files are deleted;
/// each accepted file is submitted with its own fetch time and deleted only
/// once the store reports it written. While [current] holds, the accepted
/// events are folded and the profiles cached. Returns the files deleted.
Future<int> drainPushInbox({
  required MessageStore store,
  required PushInboxDrainScope scope,
  required String root,
  required bool Function() current,
  required List<PushInboxFold> folds,
  required void Function(NostrEvent profile) cacheProfile,
}) async {
  final read = await readPushInboxFolder(
    folder: '$root/${scope.token}',
    relayUrl: scope.relayUrl,
    account: scope.lease.account,
  );
  var deleted = 0;
  Future<void> delete(String path) async {
    try {
      await File(path).delete();
      deleted++;
    } on FileSystemException {
      // Already gone.
    }
  }

  for (final path in read.refused) {
    await delete(path);
  }
  if (read.files.isEmpty) return deleted;
  final outcomes = await Future.wait([
    for (final file in read.files)
      store.submitPushed(scope.lease, file.events, fetchedAt: file.fetchedAt),
  ]);
  if (current()) {
    final seen = <String>{};
    final accepted = <NostrEvent>[];
    for (var i = 0; i < read.files.length; i++) {
      for (final event in read.files[i].events) {
        final id = event['id'] as String;
        if (outcomes[i].accepted.contains(id) && seen.add(id)) {
          accepted.add(NostrEvent.fromJson(event));
        }
      }
    }
    accepted.sort((a, b) => a.createdAt.compareTo(b.createdAt));
    if (accepted.isNotEmpty) {
      for (final fold in folds) {
        fold(scope.lease, accepted);
      }
    }
    for (final file in read.files) {
      for (final profile in file.profiles) {
        cacheProfile(NostrEvent.fromJson(profile));
      }
    }
  }
  for (var i = 0; i < read.files.length; i++) {
    if (outcomes[i].written) await delete(read.files[i].path);
  }
  return deleted;
}

/// Runs one drain at a time; a request during one runs another after it.
class PushInboxDrain {
  PushInboxDrain(this._ref);

  final Ref _ref;
  Future<void>? _running;
  bool _again = false;
  bool _disposed = false;

  void request() {
    if (_disposed) return;
    if (_running != null) {
      _again = true;
      return;
    }
    _running = _loop().whenComplete(() => _running = null);
  }

  Future<void> _loop() async {
    do {
      _again = false;
      try {
        await _run();
      } catch (error) {
        debugPrint('[PushInbox] drain failed: $error');
      }
    } while (_again && !_disposed);
  }

  MessageStoreLease? _currentLease() {
    final config = _ref.read(relayConfigProvider);
    return _ref
        .read(messageStoreProvider)
        .lease(config.baseUrl, _ref.read(myPubkeyProvider));
  }

  Future<void> _run() async {
    final root = await _ref.read(pushInboxRootProvider)();
    if (root == null || _disposed) return;
    final communities = _ref.read(communityListProvider).value;
    if (communities != null) {
      await sweepPushInbox(root, {
        for (final community in communities)
          if (!community.signedOut) ?community.pushInboxToken,
      });
    }
    if (_disposed) return;
    final store = _ref.read(messageStoreProvider);
    final scope = pushInboxDrainScope(
      ageRestricted: _ref.read(ageSignalProvider) == AgeSignalState.restricted,
      community: _ref.read(activeCommunityProvider).value,
      relayUrl: _ref.read(relayConfigProvider).baseUrl,
      account: _ref.read(myPubkeyProvider),
      store: store,
    );
    if (scope == null) return;
    await drainPushInbox(
      store: store,
      scope: scope,
      root: root,
      current: () => !_disposed && _currentLease() == scope.lease,
      folds: _ref.read(pushInboxFoldsProvider),
      cacheProfile: _ref.read(userCacheProvider.notifier).cacheProfileEvent,
    );
  }
}

/// Keeps the drain running for the app's lifetime (mounted in `app.dart`).
final pushInboxDrainProvider = Provider<PushInboxDrain?>((ref) {
  if (!pushInboxEnabled) return null;
  final drain = PushInboxDrain(ref);
  ref.onDispose(() => drain._disposed = true);
  ref.listen(appLifecycleProvider, (previous, next) {
    if (next == AppLifecycleState.resumed) drain.request();
  });
  ref.listen(activeCommunityProvider, (_, _) => drain.request());
  ref.listen(myPubkeyProvider, (_, _) => drain.request());
  ref.listen(
    ageSignalProvider,
    (_, _) => drain.request(),
    fireImmediately: true,
  );
  return drain;
});
