import 'package:kuzz/features/activity/feed_item.dart';
import 'package:kuzz/features/activity/inbox_item.dart';
import 'package:kuzz/features/activity/inbox_preload.dart';
import 'package:kuzz/features/channels/channel_preload.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

/// Records the batches the scheduler asks for.
class _RecordingPreloader extends ChannelPreloader {
  _RecordingPreloader(super.ref);

  final List<List<String>> roomBatches = [];
  final Map<String, int> roomLatestAt = {};
  final List<List<String>> threadBatches = [];

  @override
  Future<List<String>> preloadRooms(List<ChannelPreloadTarget> targets) async {
    roomBatches.add([for (final target in targets) target.channelId]);
    for (final target in targets) {
      roomLatestAt[target.channelId] = target.latestAt;
    }
    return const [];
  }

  @override
  Future<int> preloadThreads(
    List<ThreadRepliesArgs> threads, {
    Future<bool> Function()? beforeRequest,
  }) async {
    if (threads.isEmpty) return 0;
    if (beforeRequest != null && !await beforeRequest()) return 0;
    threadBatches.add([for (final thread in threads) thread.rootId]);
    return threads.length;
  }
}

FeedItem _message(String id, String channelId, int createdAt, {String? root}) =>
    FeedItem(
      id: id,
      kind: 9,
      pubkey: 'other',
      content: id,
      createdAt: createdAt,
      channelId: channelId,
      channelName: '',
      tags: [
        ['h', channelId],
        if (root != null) ['e', root, '', 'root'],
        if (root != null) ['e', root, '', 'reply'],
      ],
      category: 'activity',
    );

List<InboxPreloadTarget> _roomTargets(int count) => [
  for (var i = 0; i < count; i++)
    InboxPreloadTarget(channelId: 'room-$i', thread: null, latestAt: 100 - i),
];

void main() {
  test('targets keep display order and thread rows carry their root', () {
    final rows = collapseChannelMainRows(
      buildInboxItems([
        _message('reply', 'room-a', 300, root: 'root-1'),
        _message('top', 'room-b', 200),
      ]),
    );
    final targets = inboxPreloadTargets(rows);
    expect(targets.map((target) => target.key), ['room-a:root-1', 'room-b']);
    expect(targets.first.latestAt, 300);
  });

  test('the first wave loads the top twelve rooms at once, the rest in '
      'idle batches of twelve', () async {
    final container = ProviderContainer();
    addTearDown(container.dispose);
    final preloader = _RecordingPreloader(
      container.read(Provider((ref) => ref)),
    );
    var idleChecks = 0;
    await runInboxPreload(
      preloader,
      [
        ..._roomTargets(30),
        const InboxPreloadTarget(
          channelId: 'room-0',
          thread: ThreadRepliesArgs(channelId: 'room-0', rootId: 'root-0'),
          latestAt: 1,
        ),
      ],
      waitReady: () async => true,
      waitIdle: () async {
        idleChecks++;
        return true;
      },
    );

    expect(preloader.roomBatches.map((batch) => batch.length), [12, 12, 6]);
    expect(preloader.roomBatches.first.first, 'room-0');
    expect(preloader.threadBatches, [
      ['root-0'],
    ]);
    // Each background room batch and the background threads waited.
    expect(idleChecks, 3);
  });

  test('a busy user stops the background wave', () async {
    final container = ProviderContainer();
    addTearDown(container.dispose);
    final preloader = _RecordingPreloader(
      container.read(Provider((ref) => ref)),
    );
    await runInboxPreload(
      preloader,
      _roomTargets(30),
      waitReady: () async => true,
      waitIdle: () async => false,
    );
    expect(preloader.roomBatches, hasLength(1));
  });

  test('a busy thread does not make its room look newer', () async {
    final container = ProviderContainer();
    addTearDown(container.dispose);
    final preloader = _RecordingPreloader(
      container.read(Provider((ref) => ref)),
    );
    await runInboxPreload(
      preloader,
      const [
        InboxPreloadTarget(
          channelId: 'room',
          thread: ThreadRepliesArgs(channelId: 'room', rootId: 'root'),
          latestAt: 900,
        ),
        InboxPreloadTarget(channelId: 'room', thread: null, latestAt: 100),
        InboxPreloadTarget(
          channelId: 'thread-only',
          thread: ThreadRepliesArgs(channelId: 'thread-only', rootId: 'r2'),
          latestAt: 800,
        ),
      ],
      waitReady: () async => true,
      waitIdle: () async => true,
    );
    expect(preloader.roomLatestAt, {'room': 100, 'thread-only': 0});
  });
}
