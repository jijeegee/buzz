import 'package:buzz/features/activity/activity_provider.dart';
import 'package:buzz/features/activity/feed_item.dart';
import 'package:buzz/features/channels/channel.dart';
import 'package:buzz/features/channels/channels_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

class _EmptyActivityNotifier extends ActivityNotifier {
  @override
  Future<HomeFeedResponse> build() async => HomeFeedResponse(
    mentions: const [],
    needsAction: const [],
    activity: const [],
    agentActivity: const [],
  );
}

class _MutableChannelsNotifier extends ChannelsNotifier {
  _MutableChannelsNotifier(this._initial);

  final List<Channel> _initial;

  @override
  bool get hasLoaded => true;

  @override
  Future<List<Channel>> build() async => _initial;

  void replace(List<Channel> channels) => state = AsyncData(channels);
}

Channel _channel(String id, int lastMessageAt) => Channel(
  id: id,
  name: id,
  channelType: 'stream',
  visibility: 'open',
  description: '',
  createdBy: 'x',
  createdAt: DateTime(2025),
  memberCount: 2,
  isMember: true,
  lastMessageAt: DateTime.fromMillisecondsSinceEpoch(
    lastMessageAt * 1000,
    isUtc: true,
  ),
);

FeedItem _message(String id, String channelId, int createdAt, String pubkey) =>
    FeedItem(
      id: id,
      kind: 9,
      pubkey: pubkey,
      content: id,
      createdAt: createdAt,
      channelId: channelId,
      channelName: '',
      tags: [
        ['h', channelId],
      ],
      category: 'activity',
    );

void main() {
  test('a room re-sorts by its newest message after the view loaded, '
      'including my own send', () async {
    final channels = _MutableChannelsNotifier([
      _channel('c1', 100),
      _channel('c2', 150),
    ]);
    final refreshed = <String>[];
    final container = ProviderContainer(
      overrides: [
        activityProvider.overrideWith(_EmptyActivityNotifier.new),
        channelsProvider.overrideWith(() => channels),
        channelActivityProvider.overrideWith(
          (ref) async => [
            _message('c1-old', 'c1', 100, 'other_pk'),
            _message('c2-old', 'c2', 150, 'other_pk'),
          ],
        ),
        roomActivityRefreshProvider.overrideWith((ref, channelId) async {
          refreshed.add(channelId);
          return [
            _message('c1-old', 'c1', 100, 'other_pk'),
            _message('c1-mine', 'c1', 200, 'me_pk'),
          ];
        }),
      ],
    );
    addTearDown(container.dispose);
    final subscription = container.listen(
      conversationInboxItemsProvider,
      (_, _) {},
    );
    addTearDown(subscription.close);

    await container.read(channelsProvider.future);
    await container.read(activityProvider.future);
    await container.read(channelActivityProvider.future);
    var rows = container.read(conversationInboxItemsProvider);
    expect(rows.map((row) => row.conversationId), ['channel:c2', 'channel:c1']);
    expect(refreshed, isEmpty);

    // I send a message in c1: the live subscription advances lastMessageAt.
    channels.replace([_channel('c1', 200), _channel('c2', 150)]);
    container.read(conversationInboxItemsProvider);
    await container.read(roomActivityRefreshProvider('c1').future);
    rows = container.read(conversationInboxItemsProvider);

    expect(refreshed, ['c1']);
    expect(rows.map((row) => row.conversationId), ['channel:c1', 'channel:c2']);
    expect(rows.first.id, 'c1-mine');
  });
}
