import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';

/// The people I blocked, stored on the server and readable only by me.
///
/// Blocking is quiet: the blocked person is not told. The server stops
/// delivering their direct messages and pushes to me; in shared channels the
/// app folds their messages (see `BlockedMessageGate`). Without an account
/// (an older key-only community) the list is empty and cannot change.
class BlockedUsersNotifier extends AsyncNotifier<Set<String>> {
  @override
  Future<Set<String>> build() async {
    final origin = ref.watch(accountOriginProvider);
    if (origin == null) return const <String>{};
    final blocked = await ref.watch(accountApiProvider(origin)).listBlocks();
    return {for (final person in blocked) person.pubkey};
  }

  AccountApi _api() {
    final origin = ref.read(accountOriginProvider);
    if (origin == null) {
      throw const AccountApiException('Blocking needs a Kuzz account.');
    }
    return ref.read(accountApiProvider(origin));
  }

  /// Blocks [pubkey] on the server, then adds it here. Throws when the server
  /// refuses, leaving the list as it was.
  Future<void> block(String pubkey) async {
    final normalized = pubkey.toLowerCase();
    final api = _api();
    final known = await _settled();
    await api.block(normalized);
    state = AsyncValue.data({...known, normalized});
  }

  /// Unblocks [pubkey] on the server, then removes it here.
  Future<void> unblock(String pubkey) async {
    final normalized = pubkey.toLowerCase();
    final api = _api();
    final known = await _settled();
    await api.unblock(normalized);
    state = AsyncValue.data({...known}..remove(normalized));
  }

  /// The loaded list once any in-flight load ends, so a load finishing later
  /// cannot overwrite this change; empty when the load failed.
  Future<Set<String>> _settled() async {
    try {
      return await future;
    } on Object {
      return const <String>{};
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
