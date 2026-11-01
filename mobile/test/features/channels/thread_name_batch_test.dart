import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/thread_name_batch.dart';
import 'package:kuzz/shared/relay/relay.dart';

var _nextId = 0;

NostrEvent _name(String room, String head, String content, int createdAt) =>
    NostrEvent(
      id: 'n${(_nextId++).toString().padLeft(6, '0')}',
      pubkey: 'b' * 64,
      createdAt: createdAt,
      kind: EventKind.threadName,
      tags: [
        ['h', room],
        ['e', head],
      ],
      content: content,
      sig: '',
    );

/// A relay holding name events, answering newest first like the real one.
class _Relay {
  _Relay(this.events, {this.fail = false});

  final List<NostrEvent> events;
  final bool fail;
  final reads = <NostrFilter>[];

  Future<List<NostrEvent>> query(NostrFilter filter) async {
    reads.add(filter);
    if (fail) throw Exception('fixture relay failure');
    final matched =
        events
            .where(
              (event) =>
                  filter.tags['#h']!.contains(event.tags[0][1]) &&
                  filter.tags['#e']!.contains(event.tags[1][1]) &&
                  (filter.until == null || event.createdAt <= filter.until!),
            )
            .toList()
          ..sort(
            (a, b) => a.createdAt == b.createdAt
                ? a.id.compareTo(b.id)
                : b.createdAt.compareTo(a.createdAt),
          );
    return matched.take(filter.limit).toList();
  }
}

ThreadNameBatcher _batcher(_Relay relay, List<String> covered) =>
    ThreadNameBatcher(
      query: relay.query,
      whenRoomCovered: (roomId) async => covered.add(roomId),
      window: Duration.zero,
    );

void main() {
  test('50 rows of one room share one read with their newest names', () async {
    final events = [
      for (var i = 0; i < 50; i++) ...[
        _name('a', 'h$i', 'old $i', 10),
        _name('a', 'h$i', 'new $i', 20),
      ],
    ];
    final relay = _Relay(events);
    final covered = <String>[];
    final batcher = _batcher(relay, covered);
    final names = await Future.wait([
      for (var i = 0; i < 50; i++)
        batcher.load((channelId: 'a', headId: 'h$i')),
    ]);
    expect(relay.reads, hasLength(1));
    expect(relay.reads.single.kinds, [EventKind.threadName]);
    expect(relay.reads.single.tags['#e'], hasLength(50));
    expect(relay.reads.single.limit, threadNamePageLimit);
    expect(covered, ['a'], reason: 'the read waits for room coverage');
    for (var i = 0; i < 50; i++) {
      expect(names[i]?.content, 'new $i');
    }
  });

  test('rows in many rooms share reads of at most 100 heads', () async {
    final relay = _Relay([]);
    final batcher = _batcher(relay, []);
    final names = await Future.wait([
      for (var i = 0; i < 250; i++)
        batcher.load((channelId: 'room$i', headId: 'h$i')),
    ]);
    expect(names.every((name) => name == null), isTrue);
    expect(relay.reads, hasLength((250 / 100).ceil()));
    for (final read in relay.reads) {
      expect(read.tags['#e']!.length, lessThanOrEqualTo(100));
      expect(read.tags['#h']!.length, lessThanOrEqualTo(128));
    }
  });

  test('pages drop named heads from #e and use an inclusive until', () async {
    final events = [
      _name('a', 'quiet', 'quiet name', 1),
      for (var i = 0; i < threadNamePageLimit + 5; i++)
        _name('a', 'busy', 'busy $i', 100 + i),
    ];
    final relay = _Relay(events);
    final batcher = _batcher(relay, []);
    final busy = batcher.load((channelId: 'a', headId: 'busy'));
    final quiet = batcher.load((channelId: 'a', headId: 'quiet'));
    expect((await busy)?.content, 'busy ${threadNamePageLimit + 4}');
    expect((await quiet)?.content, 'quiet name');
    expect(relay.reads, hasLength(2));
    expect(relay.reads[1].tags['#e'], ['quiet']);
    expect(relay.reads[1].until, 105);
  });

  test('the page cap leaves the rest unresolved', () async {
    final events = [
      for (var i = 0; i < threadNamePageLimit * (threadNameMaxPages + 2); i++)
        _name('a', 'noise${i % 99}', 'n', 100000 - i),
    ];
    final relay = _Relay(events);
    final batcher = _batcher(relay, []);
    final noise = [
      for (var i = 0; i < 99; i++)
        batcher.load((channelId: 'a', headId: 'noise$i')),
    ];
    final missing = batcher.load((channelId: 'a', headId: 'never'));
    await Future.wait(noise);
    expect(await missing, isNull);
    expect(relay.reads.length, lessThanOrEqualTo(threadNameMaxPages));
  });

  test('a failed read fails every caller in it', () async {
    final batcher = _batcher(_Relay([], fail: true), []);
    final a = batcher.load((channelId: 'a', headId: 'h1'));
    final b = batcher.load((channelId: 'b', headId: 'h2'));
    await expectLater(a, throwsException);
    await expectLater(b, throwsException);
  });
}
