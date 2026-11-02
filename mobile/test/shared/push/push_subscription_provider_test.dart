import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_mutes/channel_mutes_provider.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/push/push_subscription.dart';
import 'package:kuzz/shared/push/push_subscription_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:uuid/uuid.dart';

void main() {
  const activeID = '123e4567-e89b-42d3-a456-426614174000';
  const archivedID = '123e4567-e89b-42d3-a456-426614174001';
  const nonMemberID = '123e4567-e89b-42d3-a456-426614174002';

  test(
    'derives desired subscriptions from nsec, membership, and mute state',
    () {
      final nsec = nostr.Keys.generate().nsec;
      final subscriptions = desiredKuzzPushSubscriptions(
        community: Community.create(
          name: 'Team',
          relayUrl: 'https://relay.example.com',
          nsec: nsec,
        ),
        channels: [
          channel(activeID),
          channel(archivedID, archived: true),
          channel(nonMemberID, isMember: false),
        ],
        mutedChannelIds: const [activeID, archivedID],
      );

      expect(subscriptions, isNotNull);
      expect(subscriptions, hasLength(1));
      expect(subscriptions!.single.filter.pTags, hasLength(1));
      expect(subscriptions.single.ignore, hasLength(2));
      expect(subscriptions.single.ignore.first.kinds, kuzzPushRenderableKinds);
      expect(subscriptions.single.ignore.last.hTags, [activeID]);
    },
  );

  test('returns no desired subscriptions without a signing identity', () {
    final subscriptions = desiredKuzzPushSubscriptions(
      community: Community.create(
        name: 'Team',
        relayUrl: 'https://relay.example.com',
      ),
      channels: [channel(activeID)],
      mutedChannelIds: const [],
    );

    expect(subscriptions, isNull);
  });

  testWidgets('too many chats make push unavailable without breaking the app', (
    tester,
  ) async {
    const uuid = Uuid();
    final chats = [for (var i = 0; i < 800; i++) channel(uuid.v4())];
    final community = Community.create(
      name: 'Team',
      relayUrl: 'https://relay.example.com',
      nsec: nostr.Keys.generate().nsec,
    ).copyWith(pushNotificationsEnabled: true);
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          activeCommunityProvider.overrideWith((ref) async => community),
          channelsProvider.overrideWith(() => _Channels(chats)),
          channelMutesProvider.overrideWith(_ReadyMutes.new),
        ],
        child: const MaterialApp(home: _SyncProbe()),
      ),
    );
    await tester.pumpAndSettle();

    expect(tester.takeException(), isNull);
    expect(find.text('unavailable'), findsOneWidget);
  });
}

class _SyncProbe extends ConsumerWidget {
  const _SyncProbe();

  @override
  Widget build(BuildContext context, WidgetRef ref) =>
      Text(ref.watch(pushSubscriptionSyncProvider).name);
}

class _Channels extends ChannelsNotifier {
  _Channels(this.channels);

  final List<Channel> channels;

  @override
  Future<List<Channel>> build() async => channels;
}

class _ReadyMutes extends ChannelMutesNotifier {
  @override
  ChannelMutesState build() => const ChannelMutesState(isReady: true);
}

Channel channel(String id, {bool isMember = true, bool archived = false}) =>
    Channel(
      id: id,
      name: id,
      channelType: 'dm',
      visibility: 'open',
      description: '',
      createdBy: 'author',
      createdAt: DateTime(2026),
      memberCount: 1,
      isMember: isMember,
      archivedAt: archived ? DateTime(2026) : null,
    );
