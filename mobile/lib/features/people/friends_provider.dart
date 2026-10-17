import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../pulse/pulse_models.dart';
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
/// right before writing: a change made on another device in the meantime is
/// kept instead of overwritten.
class FriendsController {
  FriendsController(this._ref);

  final Ref _ref;

  Future<void> add(String pubkey) async {
    final me = _ref.read(myPubkeyProvider);
    if (me == null) throw StateError('Not signed in');
    final normalized = pubkey.toLowerCase();
    final contacts = await _fetchLatest(me);
    if (contacts.any((entry) => entry.pubkey.toLowerCase() == normalized)) {
      _ref.invalidate(contactListProvider(me.toLowerCase()));
      return;
    }
    await _write(me, [...contacts, ContactEntry(pubkey: normalized)]);
  }

  Future<void> remove(String pubkey) async {
    final me = _ref.read(myPubkeyProvider);
    if (me == null) throw StateError('Not signed in');
    final normalized = pubkey.toLowerCase();
    final contacts = await _fetchLatest(me);
    final kept = [
      for (final entry in contacts)
        if (entry.pubkey.toLowerCase() != normalized) entry,
    ];
    if (kept.length == contacts.length) {
      _ref.invalidate(contactListProvider(me.toLowerCase()));
      return;
    }
    await _write(me, kept);
  }

  Future<List<ContactEntry>> _fetchLatest(String me) async {
    final events = await _ref
        .read(relaySessionProvider.notifier)
        .fetchHistory(NostrFilters.contactList(me.toLowerCase()));
    return contactsFromEvents(events);
  }

  Future<void> _write(String me, List<ContactEntry> contacts) async {
    final relay = SignedEventRelay.forConfig(
      session: _ref.read(relaySessionProvider.notifier),
      config: _ref.read(relayConfigProvider),
    );
    await relay.submit(
      kind: EventKind.contactList,
      content: '',
      tags: [for (final entry in contacts) entry.toTag()],
    );
    _ref.invalidate(contactListProvider(me.toLowerCase()));
  }
}

final friendsControllerProvider = Provider<FriendsController>(
  FriendsController.new,
);
