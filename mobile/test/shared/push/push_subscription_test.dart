import 'dart:convert';

import 'package:kuzz/shared/push/push_subscription.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final me = 'a' * 64;
  const channelA = '123e4567-e89b-42d3-a456-426614174000';
  const channelB = '123e4567-e89b-42d3-a456-426614174001';

  test(
    'desired subscription state round-trips with an accepted-state seam',
    () {
      final subscriptions = buildDesiredKuzzPushSubscriptions(
        myPubkey: me,
        channelIds: [channelB, channelA],
      );
      final state = KuzzPushLeaseSubscriptionState.desired(
        desired: subscriptions,
      );

      final decoded = KuzzPushLeaseSubscriptionState.fromJson(
        jsonDecode(jsonEncode(state.toJson())) as Map<String, dynamic>,
      );

      expect(decoded.authority, KuzzPushLeaseSubscriptionAuthority.desired);
      expect(decoded.accepted, isNull);
      expect(decoded.toJson(), state.toJson());
      expect(decoded.authoritative, hasLength(2));
      expect(decoded.authoritative.last.filter.hTags, [channelA, channelB]);
    },
  );

  test('accepted authority requires observed accepted subscriptions', () {
    final subscription = buildDesiredKuzzPushSubscriptions(myPubkey: me).single;

    expect(
      () => KuzzPushLeaseSubscriptionState.fromJson({
        'authority': 'accepted',
        'desired': [subscription.toJson()],
      }),
      throwsFormatException,
    );
  });

  test('persists accepted and reserved relay lease generations', () {
    final subscription = buildDesiredKuzzPushSubscriptions(myPubkey: me).single;
    final state =
        KuzzPushLeaseSubscriptionState.desired(desired: [subscription])
            .withAccepted(subscriptions: [subscription], generation: 9)
            .withReservedGeneration(10);

    final decoded = KuzzPushLeaseSubscriptionState.fromJson(state.toJson());
    expect(decoded.acceptedGeneration, 9);
    expect(decoded.generationCursor, 10);
    expect(decoded.toJson(), state.toJson());
  });

  test('a retry reserves beyond a committed but unrecorded generation', () {
    final subscription = buildDesiredKuzzPushSubscriptions(myPubkey: me).single;
    final accepted = KuzzPushLeaseSubscriptionState.desired(
      desired: [subscription],
    ).withAccepted(subscriptions: [subscription], generation: 4);
    final committed = accepted.withReservedGeneration(5);

    final recovered = KuzzPushLeaseSubscriptionState.fromJson(
      committed.toJson(),
    ).withReservedGeneration(6);

    expect(recovered.acceptedGeneration, 4);
    expect(recovered.generationCursor, 6);
  });

  test('builds aligned self and unmuted channel subscriptions', () {
    final subscriptions = buildDesiredKuzzPushSubscriptions(
      myPubkey: me.toUpperCase(),
      channelIds: [channelB, channelA],
      mutedChannelIds: [channelB, '123e4567-e89b-42d3-a456-426614174099'],
    );

    expect(subscriptions, hasLength(2));
    expect(subscriptions.first.filter.kinds, kuzzPushSelfDirectedKinds);
    expect(subscriptions.first.filter.kinds, isNot(contains(7)));
    expect(subscriptions.first.filter.pTags, [me]);
    expect(subscriptions.last.filter.kinds, kuzzPushChannelKinds);
    expect(subscriptions.last.filter.hTags, [channelA]);
    expect(subscriptions.first.ignore, hasLength(2));
    expect(subscriptions.first.ignore.first.kinds, kuzzPushRenderableKinds);
    expect(subscriptions.first.ignore.last.hTags, [channelB]);
    expect(
      subscriptions.first.suppress?.pTagsMax,
      kuzzPushHellthreadParticipantLimit,
    );
  });

  test('chunks channel subscriptions to relay limits deterministically', () {
    final channels = [
      for (var i = 0; i < 51; i++)
        '00000000-0000-4000-8000-${i.toString().padLeft(12, '0')}',
    ]..shuffle();

    final subscriptions = buildDesiredKuzzPushSubscriptions(
      myPubkey: me,
      channelIds: channels,
    );

    expect(subscriptions, hasLength(3));
    expect(subscriptions[1].filter.hTags, hasLength(50));
    expect(subscriptions[2].filter.hTags, hasLength(1));
    expect(
      subscriptions[1].filter.hTags,
      orderedEquals([...subscriptions[1].filter.hTags!]..sort()),
    );
  });

  test('rejects malformed and unsupported subscription fields', () {
    final valid = buildDesiredKuzzPushSubscriptions(
      myPubkey: me,
    ).single.toJson();

    expect(
      () => buildDesiredKuzzPushSubscriptions(
        myPubkey: me,
        channelIds: const ['not-a-channel'],
      ),
      throwsFormatException,
    );

    expect(
      () => KuzzPushSubscription.fromJson({...valid, 'unexpected': true}),
      throwsFormatException,
    );
    expect(
      () => KuzzPushSubscription.fromJson({
        'filter': {
          'kinds': [7],
          '#p': [me],
        },
        'class': 'default',
      }),
      throwsFormatException,
    );
    expect(
      () => KuzzPushSubscription.fromJson({
        'filter': {
          'kinds': [9],
          '#p': ['not-a-pubkey'],
        },
        'class': 'default',
      }),
      throwsFormatException,
    );
  });
}
