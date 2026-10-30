import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:kuzz/shared/profile/profile_snapshot.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _relay = 'https://relay.example';
const _me = 'aa00000000000000000000000000000000000000000000000000000000000001';
const _other =
    'bb00000000000000000000000000000000000000000000000000000000000002';
const _them =
    'dd00000000000000000000000000000000000000000000000000000000000004';

/// Other people's profiles saved in the account's store file
/// (docs/message-sync.md#launch-order): drawn at launch, asked for again
/// once, replaced by the relay's answer, and never drawn for another account.
void main() {
  sqfliteFfiInit();

  late Directory dir;
  late MessageStore store;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('user_cache_snapshot');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
  });

  tearDown(() async {
    await store.forget(_relay, _me);
    await store.forget(_relay, _other);
    await dir.delete(recursive: true);
  });

  ProviderContainer containerFor(String pubkey, _ProfileSession session) {
    final container = ProviderContainer(
      overrides: [
        relayConfigProvider.overrideWith(() => _FakeRelayConfigNotifier()),
        relaySessionProvider.overrideWith(() => session),
        messageStoreProvider.overrideWithValue(store),
        myPubkeyProvider.overrideWithValue(pubkey),
      ],
    );
    addTearDown(container.dispose);
    return container;
  }

  Future<void> settle() async {
    for (var i = 0; i < 5; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
  }

  test('saved profiles draw at launch for their account only', () async {
    await store.writeSnapshots(store.lease(_relay, _me), {
      '$profileSnapshotPrefix$_them': encodeProfileSnapshot(
        const UserProfile(pubkey: _them, displayName: 'Saved Mina'),
        createdAt: 1,
        eventId: 'old',
      ),
    });

    final mine = containerFor(_me, _ProfileSession());
    mine.read(userCacheProvider);
    final theirs = containerFor(_other, _ProfileSession());
    theirs.read(userCacheProvider);
    await settle();

    expect(mine.read(userCacheProvider)[_them]?.displayName, 'Saved Mina');
    expect(theirs.read(userCacheProvider), isEmpty);
  });

  test("a saved profile is asked for again once and the relay's answer "
      'replaces and saves it', () async {
    await store.writeSnapshots(store.lease(_relay, _me), {
      '$profileSnapshotPrefix$_them': encodeProfileSnapshot(
        const UserProfile(pubkey: _them, displayName: 'Saved Mina'),
        createdAt: 1,
        eventId: 'old',
      ),
    });
    final session = _ProfileSession(
      answer: [_profile(_them, 'Mina', createdAt: 5)],
    );
    final container = containerFor(_me, session);
    final cache = container.read(userCacheProvider.notifier);
    await settle();

    // Drawn at once from the saved profile, then asked for.
    expect(cache.get(_them)?.displayName, 'Saved Mina');
    await settle();
    expect(session.requests, 1);
    expect(container.read(userCacheProvider)[_them]?.displayName, 'Mina');

    // Asked for once only.
    cache.get(_them);
    await settle();
    expect(session.requests, 1);

    // The relay's answer is saved for the next launch.
    await store.flushed();
    final saved = await store.readSnapshots(
      store.lease(_relay, _me),
      profileSnapshotPrefix,
      limit: 10,
    );
    expect(
      decodeProfileSnapshot(
        saved['$profileSnapshotPrefix$_them'],
      )?.profile.displayName,
      'Mina',
    );
  });
}

NostrEvent _profile(String pubkey, String name, {required int createdAt}) =>
    NostrEvent(
      id: 'profile-$createdAt',
      pubkey: pubkey,
      createdAt: createdAt,
      kind: 0,
      tags: const [],
      content: jsonEncode({'name': name}),
      sig: 'sig',
    );

class _ProfileSession extends RelaySessionNotifier {
  _ProfileSession({this.answer = const []});

  final List<NostrEvent> answer;
  int requests = 0;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    if (!filter.kinds.contains(0)) return const [];
    requests++;
    final authors = filter.authors ?? const [];
    return [
      for (final event in answer)
        if (authors.contains(event.pubkey)) event,
    ];
  }
}

class _FakeRelayConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: _relay, nsec: null);
}
