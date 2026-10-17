import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/relay/relay.dart';

/// The people I blocked, stored on the server and readable only by me.
///
/// Blocking is quiet: the blocked person is not told. The server stops
/// delivering their direct messages and pushes to me; in shared channels the
/// app folds their messages (see `BlockedMessageGate`). Without an account
/// (an older key-only community) the list is empty and cannot change.
///
/// The list belongs to the signed-in account: switching accounts or signing
/// out rebuilds it. A failed load is retried when the relay reconnects or
/// the app returns to the foreground.
class BlockedUsersNotifier extends AsyncNotifier<Set<String>> {
  @override
  Future<Set<String>> build() async {
    final origin = ref.watch(accountOriginProvider);
    final me = ref.watch(myPubkeyProvider);
    if (origin == null || me == null || me.isEmpty) return const <String>{};
    final api = ref.watch(accountApiProvider(origin));

    void retryIfFailed() {
      if (!state.hasError) return;
      // Deferred: invalidating from inside a listener of this provider's
      // own dependencies (or during a resume callback) is not allowed.
      Future.microtask(() {
        if (ref.mounted && state.hasError) ref.invalidateSelf();
      });
    }

    ref.listen(relaySessionProvider.select((s) => s.status), (prev, next) {
      if (next == SessionStatus.connected && prev != SessionStatus.connected) {
        retryIfFailed();
      }
    });
    ref.listen(appLifecycleProvider, (prev, next) {
      if (next == AppLifecycleState.resumed &&
          prev != AppLifecycleState.resumed) {
        retryIfFailed();
      }
    });

    return _load(api);
  }

  static Future<Set<String>> _load(AccountApi api) async {
    final blocked = await api.listBlocks();
    return {for (final person in blocked) person.pubkey};
  }

  AccountApi _api() {
    final origin = ref.read(accountOriginProvider);
    if (origin == null) {
      throw const AccountApiException(
        'Blocking needs a Kuzz account.',
        statusCode: 403,
      );
    }
    return ref.read(accountApiProvider(origin));
  }

  /// Blocks [pubkey] on the server, then reloads the list from it. Throws
  /// when the server refuses, leaving the list as it was.
  Future<void> block(String pubkey) async {
    final normalized = pubkey.toLowerCase();
    final api = _api();
    await api.block(normalized);
    await _reload(api, (known) => {...known, normalized});
  }

  /// Unblocks [pubkey] on the server, then reloads the list from it.
  Future<void> unblock(String pubkey) async {
    final normalized = pubkey.toLowerCase();
    final api = _api();
    await api.unblock(normalized);
    await _reload(api, (known) => {...known}..remove(normalized));
  }

  /// The server's list after a change. If that read fails, the last good
  /// list with [applied] stands in, so the change still shows; with no good
  /// list at all only the change is known, and the list reloads later.
  Future<void> _reload(
    AccountApi api,
    Set<String> Function(Set<String> known) applied,
  ) async {
    try {
      final fresh = await _load(api);
      if (ref.mounted) state = AsyncValue.data(fresh);
    } catch (_) {
      if (!ref.mounted) return;
      state = AsyncValue.data(applied(state.value ?? const <String>{}));
    }
  }
}

final blockedUsersProvider =
    AsyncNotifierProvider<BlockedUsersNotifier, Set<String>>(
      BlockedUsersNotifier.new,
      retry: (_, _) => null,
    );

/// The blocked pubkeys known right now; empty while loading or after a
/// failed load (nothing is hidden that the server has not confirmed).
final blockedPubkeysProvider = Provider<Set<String>>(
  (ref) => ref.watch(blockedUsersProvider).value ?? const <String>{},
);
