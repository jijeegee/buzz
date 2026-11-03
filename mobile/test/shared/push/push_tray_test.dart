import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/push/push_tray.dart';
import 'package:kuzz/shared/read_state/read_state_provider.dart';

void main() {
  final head = 'a' * 64;
  const room = 'room-1';

  DeliveredPushNotification note(
    String key, {
    String eventId = 'e1',
    String communityId = 'c1',
    String channelId = room,
    String? thread,
    int createdAt = 100,
  }) => DeliveredPushNotification(
    key: key,
    eventId: eventId,
    communityId: communityId,
    channelId: channelId,
    threadRootId: thread,
    createdAt: createdAt,
  );

  ReadStateState state(
    Map<String, int> contexts, {
    bool ready = true,
    Map<String, String> forced = const {},
  }) => ReadStateState(
    isReady: ready,
    pubkey: 'me',
    contexts: contexts,
    version: 1,
    forcedUnreadContexts: forced,
  );

  List<String> keys(
    List<DeliveredPushNotification> delivered,
    ReadStateState readState,
  ) => readPushNotificationKeys(
    delivered,
    readState: readState,
    communityId: 'c1',
  );

  test('a room message goes once the room is read up to it', () {
    final delivered = [
      note('old', createdAt: 100),
      note('new', createdAt: 200),
    ];
    expect(keys(delivered, state({room: 150})), ['old']);
    expect(keys(delivered, state({room: 200})), ['old', 'new']);
    expect(keys(delivered, state({})), isEmpty);
  });

  test('a thread reply goes with its thread, its own mark or the room', () {
    final reply = note('r', eventId: 'r1', thread: head, createdAt: 100);
    expect(keys([reply], state({'thread:$head': 100})), ['r']);
    expect(keys([reply], state({'msg:r1': 100})), ['r']);
    expect(keys([reply], state({room: 100})), ['r']);
    expect(keys([reply], state({'thread:${'b' * 64}': 500})), isEmpty);
  });

  test('marked unread on purpose stays', () {
    final reply = note('r', eventId: 'r1', thread: head, createdAt: 100);
    final read = {room: 500};
    expect(keys([reply], state(read, forced: {room: room})), isEmpty);
    expect(keys([reply], state(read, forced: {'thread:$head': room})), isEmpty);
    expect(keys([reply], state(read, forced: {'msg:r1': room})), isEmpty);
  });

  test('other accounts and an unready read state are left alone', () {
    expect(keys([note('x', communityId: 'c2')], state({room: 500})), isEmpty);
    expect(keys([note('x')], state({room: 500}, ready: false)), isEmpty);
  });

  test('native rows parse, malformed ones are dropped', () {
    final parsed = DeliveredPushNotification.fromNative({
      'key': 'k',
      'eventId': 'e',
      'communityId': 'c1',
      'channelId': room,
      'createdAt': 10,
      'threadRootId': head,
    });
    expect(parsed?.threadRootId, head);
    expect(
      DeliveredPushNotification.fromNative({
        'key': 'k',
        'eventId': 'e',
        'communityId': 'c1',
        'channelId': room,
        'createdAt': 10,
        'threadRootId': 'bad',
      })?.threadRootId,
      isNull,
    );
    expect(
      DeliveredPushNotification.fromNative({
        'key': 'k',
        'eventId': 'e',
        'communityId': 'c1',
        'channelId': room,
      }),
      isNull,
    );
  });
}
