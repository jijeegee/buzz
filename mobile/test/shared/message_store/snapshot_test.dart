import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_snapshot_codec.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/profile/profile_snapshot.dart';
import 'package:kuzz/shared/profile/user_profile.dart';

const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _other =
    'bb00000000000000000000000000000000000000000000000000000000000002';
const _relay = 'wss://relay.example';
const _elsewhere = 'wss://other.example';

void main() {
  sqfliteFfiInit();
  late Directory dir;
  late MessageStore store;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('snapshot');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
  });

  tearDown(() async {
    await store.forget(_relay, _account);
    await store.forget(_relay, _other);
    await store.forget(_elsewhere, _account);
    await dir.delete(recursive: true);
  });

  test('a saved value belongs to one account and server only', () async {
    final mine = store.lease(_relay, _account)!;
    await store.writeSnapshots(mine, {'channels': 'mine'});

    expect(await store.readSnapshot(mine, 'channels'), 'mine');
    expect(
      await store.readSnapshot(store.lease(_relay, _other), 'channels'),
      isNull,
    );
    expect(
      await store.readSnapshot(store.lease(_elsewhere, _account), 'channels'),
      isNull,
    );
  });

  test('sign-out wipes the saved values with the file', () async {
    final lease = store.lease(_relay, _account)!;
    await store.writeSnapshots(lease, {'channels': 'list', 'profile:x': 'x'});
    await store.forget(_relay, _account);

    // The ended lease writes and reads nothing.
    await store.writeSnapshots(lease, {'channels': 'late'});
    final again = store.lease(_relay, _account)!;
    expect(await store.readSnapshot(again, 'channels'), isNull);
    expect(await store.readSnapshots(again, 'profile:', limit: 10), isEmpty);
  });

  test('saved profiles keep only the newest under the cap', () async {
    final lease = store.lease(_relay, _account)!;
    for (var i = 0; i < 5; i++) {
      await store.writeSnapshots(
        lease,
        {'profile:$i': '$i'},
        trimPrefix: 'profile:',
        keep: 3,
      );
      // updated_at is in milliseconds; keep the saves apart.
      await Future<void>.delayed(const Duration(milliseconds: 3));
    }
    await store.writeSnapshots(lease, {'self_profile': 'me'});

    final saved = await store.readSnapshots(lease, 'profile:', limit: 10);
    expect(saved.keys, ['profile:4', 'profile:3', 'profile:2']);
    expect(await store.readSnapshot(lease, 'self_profile'), 'me');
  });

  test('the channel list and a profile read back as saved', () {
    final channel = Channel(
      id: 'room',
      name: 'general',
      channelType: 'stream',
      visibility: 'open',
      description: 'about',
      createdBy: 'c' * 64,
      createdAt: DateTime.utc(2026, 1, 2),
      memberCount: 4,
      lastMessageAt: DateTime.utc(2026, 3, 4),
      participantPubkeys: const ['d'],
      isMember: true,
      groupChat: true,
    );
    final list = decodeChannelListSnapshot(
      encodeChannelListSnapshot([channel]),
    )!;
    expect(list.single.id, 'room');
    expect(list.single.name, 'general');
    expect(list.single.createdAt, channel.createdAt);
    expect(list.single.lastMessageAt, channel.lastMessageAt);
    expect(list.single.participantPubkeys, ['d']);
    expect(list.single.isMember, isTrue);
    expect(list.single.groupChat, isTrue);
    expect(decodeChannelListSnapshot('{"v":0,"channels":[]}'), isNull);

    final saved = decodeProfileSnapshot(
      encodeProfileSnapshot(
        const UserProfile(pubkey: 'ABC', displayName: 'Mina'),
        createdAt: 7,
        eventId: 'e',
      ),
    )!;
    expect(saved.profile.pubkey, 'abc');
    expect(saved.profile.displayName, 'Mina');
    expect(saved.createdAt, 7);
    expect(decodeProfileSnapshot('not json'), isNull);
  });
}
