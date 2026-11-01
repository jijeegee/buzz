import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/push/push_inbox.dart';

void main() {
  late Directory root;
  final token = 'ab' * 16;
  final now = DateTime.fromMillisecondsSinceEpoch(1700000000500);

  setUp(() async {
    root = await Directory.systemTemp.createTemp('push-inbox-cursor');
  });
  tearDown(() => root.delete(recursive: true));

  Community community({
    bool push = true,
    bool out = false,
    String? inbox = 'set',
  }) => Community(
    id: 'c1',
    name: 'C',
    relayUrl: 'https://relay.example',
    pubkey: 'a' * 64,
    pushNotificationsEnabled: push,
    pushInboxToken: inbox == 'set' ? token : inbox,
    signedOut: out,
    addedAt: DateTime.fromMillisecondsSinceEpoch(0),
  );

  File cursor() => File('${root.path}/$token/cursor');

  test('pause while connected moves the cursor to now, atomically', () async {
    await notePushInboxPause(
      root: root.path,
      community: community(),
      connected: true,
      now: now,
    );
    expect(await cursor().readAsString(), '1700000000');
    final names = Directory(
      '${root.path}/$token',
    ).listSync().map((e) => e.uri.pathSegments.last);
    expect(names, ['cursor'], reason: 'no temporary file is left');

    await notePushInboxPause(
      root: root.path,
      community: community(),
      connected: true,
      now: now.add(const Duration(seconds: 5)),
    );
    expect(await cursor().readAsString(), '1700000005');
  });

  test(
    'nothing is written when not connected or without a live token',
    () async {
      for (final (connected, c) in [
        (false, community()),
        (true, community(push: false)),
        (true, community(out: true)),
        (true, community(inbox: null)),
        (true, community(inbox: '../x')),
        (true, null),
      ]) {
        await notePushInboxPause(
          root: root.path,
          community: c,
          connected: connected,
          now: now,
        );
      }
      await notePushInboxPause(
        root: null,
        community: community(),
        connected: true,
        now: now,
      );
      expect(root.listSync(), isEmpty);
    },
  );
}
