import 'dart:io';

import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';

/// The legacy persisted channel heads, removed once rooms read the message
/// database.
void main() {
  group('ChannelHeadCache', () {
    late Directory root;
    late ChannelHeadCache cache;

    setUp(() async {
      root = await Directory.systemTemp.createTemp('channel_heads_test');
      cache = ChannelHeadCache(() async => root);
    });

    tearDown(() async {
      if (await root.exists()) await root.delete(recursive: true);
    });

    test('round-trips a head per account and relay', () async {
      await cache.store(_scope, _channelId, [_event(id: 'a', createdAt: 1)]);

      final loaded = await cache.load(_scope, _channelId);
      expect(loaded?.map((event) => event.toJson()), [
        _event(id: 'a', createdAt: 1).toJson(),
      ]);
      expect(await cache.load(_otherAccount, _channelId), isNull);
      expect(await cache.load(_scope, 'other-channel'), isNull);
    });

    test('the store time ignores reads that mark a head as used', () async {
      expect(await cache.storedAt(_scope, _channelId), isNull);
      final before = DateTime.now().subtract(const Duration(seconds: 1));
      await cache.store(_scope, _channelId, [_event(id: 'a', createdAt: 1)]);
      final storedAt = await cache.storedAt(_scope, _channelId);
      expect(storedAt!.isAfter(before), isTrue);

      _entryFiles(root).single.setLastModifiedSync(DateTime(2100));
      expect(await cache.load(_scope, _channelId), isNotNull);
      expect(await cache.storedAt(_scope, _channelId), storedAt);
    });

    test('keeps only the most recently used channels', () async {
      final small = ChannelHeadCache(() async => root, maxChannels: 3);
      for (var i = 0; i < 3; i++) {
        await small.store(_scope, 'channel-$i', [_event(id: 'e$i')]);
      }
      // Order the entries deterministically: channel-0 is least recent.
      final base = DateTime(2020);
      for (var i = 0; i < 3; i++) {
        _entryFiles(root)
            .singleWhere(
              (file) => file.readAsStringSync().contains('"channel-$i"'),
            )
            .setLastModifiedSync(base.add(Duration(minutes: i)));
      }
      // Reading a head marks it as recently used.
      expect(await small.load(_scope, 'channel-0'), isNotNull);

      await small.store(_scope, 'channel-new', [_event(id: 'new')]);

      expect(_entryFiles(root), hasLength(3));
      expect(await small.load(_scope, 'channel-1'), isNull);
      expect(await small.load(_scope, 'channel-0'), isNotNull);
      expect(await small.load(_scope, 'channel-2'), isNotNull);
      expect(await small.load(_scope, 'channel-new'), isNotNull);
    });

    test('drops an oversized head instead of keeping a stale one', () async {
      await cache.store(_scope, _channelId, [_event(id: 'small')]);
      final huge = 'x' * (ChannelHeadCache.maxEntryBytes + 1);
      await cache.store(_scope, _channelId, [_event(id: 'big', content: huge)]);

      expect(await cache.load(_scope, _channelId), isNull);
    });

    test('treats a corrupt entry as a miss and removes it', () async {
      await cache.store(_scope, _channelId, [_event(id: 'a')]);
      final file = _entryFiles(root).single..writeAsStringSync('{"v":1,');

      expect(await cache.load(_scope, _channelId), isNull);
      expect(file.existsSync(), isFalse);
    });

    test('a clear drops writes that were requested before it', () async {
      final write = cache.store(_scope, _channelId, [_event(id: 'a')]);
      final clear = cache.clearAll();
      await Future.wait([write, clear]);

      expect(await cache.load(_scope, _channelId), isNull);
      expect(root.existsSync(), isFalse);
    });

    test('clears one relay or everything', () async {
      const otherRelay = ChannelHeadScope(
        relayUrl: 'https://other.example',
        pubkey: 'alice',
      );
      await cache.store(_scope, _channelId, [_event(id: 'a')]);
      await cache.store(_otherAccount, _channelId, [_event(id: 'b')]);
      await cache.store(otherRelay, _channelId, [_event(id: 'c')]);

      await cache.clearRelay(_scope.relayUrl);
      expect(await cache.load(_scope, _channelId), isNull);
      expect(await cache.load(_otherAccount, _channelId), isNull);
      expect(await cache.load(otherRelay, _channelId), isNotNull);

      await cache.clearAll();
      expect(await cache.load(otherRelay, _channelId), isNull);
    });
  });
}

const _channelId = '11111111-1111-4111-8111-111111111111';
const _scope = ChannelHeadScope(
  relayUrl: 'https://relay.example',
  pubkey: 'alice',
);
const _otherAccount = ChannelHeadScope(
  relayUrl: 'https://relay.example',
  pubkey: 'bob',
);

List<File> _entryFiles(Directory root) => root
    .listSync(recursive: true)
    .whereType<File>()
    .where((file) => file.path.endsWith('.json'))
    .toList();

NostrEvent _event({required String id, int createdAt = 1, String? content}) =>
    NostrEvent(
      id: id,
      pubkey: 'alice',
      createdAt: createdAt,
      kind: EventKind.streamMessageV2,
      tags: const [
        ['h', _channelId],
      ],
      content: content ?? id,
      sig: 'sig',
    );
