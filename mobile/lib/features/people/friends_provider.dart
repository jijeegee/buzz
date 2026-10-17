import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../pulse/pulse_provider.dart';

/// My friends: the people in my follow list (kind 3). Adding a friend is
/// one-way, with no request or acceptance, like desktop's People screen.
final friendIdsProvider = Provider<AsyncValue<Set<String>>>((ref) {
  final me = ref.watch(myPubkeyProvider);
  if (me == null || me.isEmpty) return const AsyncValue.data(<String>{});
  return ref
      .watch(contactListProvider(me.toLowerCase()))
      .whenData(
        (contacts) => {
          for (final entry in contacts) entry.pubkey.toLowerCase(),
        },
      );
});

/// Adds and removes friends one person at a time. The follow list is
/// replaced as a whole on the relay, so each change re-reads the latest list
/// right before writing and changes only that one person: everyone else,
/// their relay hints and petnames, other tags and the list's content are
/// kept exactly, including changes made on another device.
class FriendsController {
  FriendsController(this._ref);

  final Ref _ref;

  Future<void> add(String pubkey) async {
    final normalized = pubkey.toLowerCase();
    await _change((tags) {
      if (tags.any((tag) => _isFriendTag(tag, normalized))) return null;
      return [
        ...tags,
        ['p', normalized],
      ];
    });
  }

  Future<void> remove(String pubkey) async {
    final normalized = pubkey.toLowerCase();
    await _change((tags) {
      final kept = [
        for (final tag in tags)
          if (!_isFriendTag(tag, normalized)) tag,
      ];
      return kept.length == tags.length ? null : kept;
    });
  }

  static bool _isFriendTag(List<String> tag, String pubkey) =>
      tag.length >= 2 && tag[0] == 'p' && tag[1].toLowerCase() == pubkey;

  /// Applies [edit] to the latest list's tags; a null result writes nothing.
  Future<void> _change(
    List<List<String>>? Function(List<List<String>> tags) edit,
  ) async {
    final me = _ref.read(myPubkeyProvider);
    if (me == null) throw StateError('Not signed in');
    final key = me.toLowerCase();
    final session = _ref.read(relaySessionProvider.notifier);
    final relay = SignedEventRelay.forConfig(
      session: session,
      config: _ref.read(relayConfigProvider),
    );
    final events = await session.fetchHistory(NostrFilters.contactList(key));
    final latest = events.isEmpty
        ? null
        : events.reduce((a, b) => b.createdAt > a.createdAt ? b : a);
    final tags = [
      for (final tag in latest?.tags ?? const <List<String>>[]) [...tag],
    ];
    final edited = edit(tags);
    if (edited != null) {
      await relay.submit(
        kind: EventKind.contactList,
        content: latest?.content ?? '',
        tags: edited,
      );
    }
    _ref.invalidate(contactListProvider(key));
  }
}

final friendsControllerProvider = Provider<FriendsController>(
  FriendsController.new,
);
