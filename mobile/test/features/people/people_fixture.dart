import 'dart:async';

import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/misc.dart' show Override;
import 'package:kuzz/features/people/people_favorites.dart';
import 'package:kuzz/features/pulse/pulse_provider.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:nostr/nostr.dart' as nostr;

const peopleOrigin = 'https://relay.test';
final ann = 'a' * 64;
final bob = 'b' * 64;
final cat = 'c' * 64;

/// A connected relay session that answers kind 3 reads from [contacts] and
/// records every publish.
class PeopleSession extends RelaySessionNotifier {
  final List<NostrEvent> contacts = [];
  final List<NostrEvent> published = [];
  final List<NostrFilter> reads = [];
  Object? publishError;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    reads.add(filter);
    if (filter.kinds.contains(EventKind.contactList)) {
      return [...contacts];
    }
    return const [];
  }

  /// Live subscriptions still open, by filter.
  final Map<NostrFilter, void Function(NostrEvent)> live = {};

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    live[filter] = onEvent;
    return () => live.remove(filter);
  }

  /// Delivers [event] to every open subscription for its kind.
  void deliver(NostrEvent event) {
    for (final entry in [...live.entries]) {
      if (entry.key.kinds.contains(event.kind)) entry.value(event);
    }
  }

  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final error = publishError;
    if (error != null) throw error;
    published.add(event);
    if (event.kind == EventKind.contactList) contacts.add(event);
    return event;
  }

  /// Stores a kind 3 list following [pubkeys].
  void follow(
    String author,
    List<String> pubkeys, {
    int createdAt = 100,
    List<List<String>>? tags,
    String content = '',
  }) {
    contacts.add(
      NostrEvent(
        id: 'k$createdAt'.padRight(64, '0'),
        pubkey: author,
        createdAt: createdAt,
        kind: EventKind.contactList,
        tags:
            tags ??
            [
              for (final pk in pubkeys) ['p', pk],
            ],
        content: content,
        sig: 'sig',
      ),
    );
  }

  /// Simulates the relay dropping and coming back.
  void setStatus(SessionStatus status) => state = SessionState(status: status);
}

/// An app lifecycle that tests move by hand (no platform listeners).
class ManualLifecycle extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;

  void set(AppLifecycleState next) => state = next;
}

class FixedConfig extends RelayConfigNotifier {
  FixedConfig(this.config);

  final RelayConfig config;

  @override
  RelayConfig build() => config;
}

/// The account endpoints People uses, in memory.
class FakePeopleApi extends Fake implements AccountApi {
  final Set<String> blocked = {};
  final Map<String, PersonSummary> directory = {};
  List<PersonSummary> shared = [];
  final List<String> calls = [];
  Object? blockError;

  /// How many of the next list reads fail.
  int listFailures = 0;
  Object? lookupError;
  Completer<void>? holdLookup;
  AccountProfile profile = const AccountProfile(
    principalId: 'p',
    displayName: 'Me',
    username: 'me',
  );
  Object? usernameError;

  @override
  String get origin => peopleOrigin;

  @override
  Future<AccountProfile> me() async => profile;

  @override
  Future<List<BlockedPerson>> listBlocks() async {
    calls.add('list');
    if (listFailures > 0) {
      listFailures--;
      throw const AccountApiException('offline');
    }
    return [for (final pk in blocked) BlockedPerson(pubkey: pk)];
  }

  @override
  Future<void> block(String pubkey) async {
    calls.add('block $pubkey');
    final error = blockError;
    if (error != null) throw error;
    blocked.add(pubkey);
  }

  @override
  Future<void> unblock(String pubkey) async {
    calls.add('unblock $pubkey');
    blocked.remove(pubkey);
  }

  @override
  Future<PersonSummary?> lookupPerson(String query) async {
    calls.add('lookup $query');
    await holdLookup?.future;
    final error = lookupError;
    if (error != null) throw error;
    return directory[query.trim().toLowerCase()];
  }

  @override
  Future<List<PersonSummary>> sharedPeople(
    String query, {
    int limit = 50,
  }) async {
    calls.add('shared $query');
    return [
      for (final person in shared)
        if ((person.displayName ?? '').toLowerCase().startsWith(
          query.toLowerCase(),
        ))
          person,
    ];
  }

  @override
  Future<AccountProfile> setUsername(String? username) async {
    calls.add('username $username');
    final error = usernameError;
    if (error != null) throw error;
    profile = AccountProfile(
      principalId: profile.principalId,
      displayName: profile.displayName,
      username: username,
      discoverable: profile.discoverable,
    );
    return profile;
  }

  @override
  Future<void> setDiscoverable(bool discoverable) async {
    calls.add('discoverable $discoverable');
    profile = AccountProfile(
      principalId: profile.principalId,
      displayName: profile.displayName,
      username: profile.username,
      discoverable: discoverable,
    );
  }
}

/// Favorites kept in memory, with no relay.
class MemoryFavorites extends PeopleFavoritesNotifier {
  MemoryFavorites([this.initial = const {}]);

  final Set<String> initial;

  @override
  Set<String> build() => {...initial};

  @override
  bool add(String pubkey) {
    state = {...state, pubkey.toLowerCase()};
    return true;
  }

  @override
  void remove(String pubkey) {
    state = {...state}..remove(pubkey.toLowerCase());
  }
}

/// A signed-in user with a real key, so kind 3 and report events sign.
final peopleKeys = nostr.Keys.generate();

/// Provider overrides for People: a signed-in key, [session], [api] as the
/// account (null for a key-only community), and [profiles] in the cache.
List<Override> peopleOverrides({
  required PeopleSession session,
  FakePeopleApi? api,
  Set<String> favorites = const {},
  Map<String, String> names = const {},
  List<String> agents = const [],
}) => [
  relayConfigProvider.overrideWith(
    () => FixedConfig(
      RelayConfig(baseUrl: 'https://relay.test', nsec: peopleKeys.nsec),
    ),
  ),
  relaySessionProvider.overrideWith(() => session),
  appLifecycleProvider.overrideWith(ManualLifecycle.new),
  accountOriginProvider.overrideWithValue(api == null ? null : peopleOrigin),
  if (api != null) accountApiProvider.overrideWith((ref, _) => api),
  peopleFavoritesProvider.overrideWith(() => MemoryFavorites(favorites)),
  agentPubkeysProvider.overrideWith((ref) async => agents),
  userCacheProvider.overrideWith(
    () => SeededUserCache({
      for (final entry in names.entries)
        entry.key: UserProfile(pubkey: entry.key, displayName: entry.value),
    }),
  ),
];

/// A user cache that starts with [seed] and never fetches.
class SeededUserCache extends UserCacheNotifier {
  SeededUserCache(this.seed);

  final Map<String, UserProfile> seed;

  @override
  Map<String, UserProfile> build() => {...seed};

  @override
  Future<bool> preload(List<String> pubkeys) async => true;
}
