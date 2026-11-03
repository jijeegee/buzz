import 'dart:convert';
import 'dart:io';
import 'dart:ui' show Locale;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/l10n/l10n.dart';
import 'package:kuzz/shared/push/push_fcm_background.dart';
import 'package:kuzz/shared/push/push_inbox_writer.dart';
import 'package:kuzz/shared/push/push_subscription.dart';
import 'package:nostr/nostr.dart' as nostr;

/// The Android wake (stage B A3.3) against a mock relay `/query`.
void main() {
  final me = nostr.Keys.generate();
  final alice = nostr.Keys.generate();
  final token = 'cd' * 16;
  final now = DateTime.fromMillisecondsSinceEpoch(1700000000000);
  final nowSeconds = now.millisecondsSinceEpoch ~/ 1000;
  late Directory root;

  setUp(() async {
    root = await Directory.systemTemp.createTemp('push-wake');
  });
  tearDown(() => root.delete(recursive: true));

  Map<String, dynamic> signed(
    nostr.Keys keys, {
    int kind = 9,
    required int at,
    String content = 'hello',
    String room = '11111111-1111-4111-8111-111111111111',
    List<List<String>> extraTags = const [],
  }) {
    final event = nostr.Event.from(
      kind: kind,
      content: content,
      secretKey: keys.secret,
      createdAt: at,
      tags: [
        if (kind != 0) ['h', room],
        if (kind != 0) ['p', me.public],
        ...extraTags,
      ],
    );
    return {
      'id': event.id,
      'pubkey': event.pubkey,
      'created_at': event.createdAt,
      'kind': event.kind,
      'tags': event.tags,
      'content': event.content,
      'sig': event.sig,
    };
  }

  Community community({
    String id = 'c1',
    String name = 'Team',
    bool tokenAuth = false,
    bool signedOut = false,
    String? inboxToken,
  }) {
    final subscription = KuzzPushSubscription(
      filter: KuzzPushFilter(
        kinds: const [9, 40002, 45001, 45003],
        pTags: [me.public],
      ),
      notificationClass: 'default',
    );
    return Community(
      id: id,
      name: name,
      relayUrl: 'wss://relay.example',
      pubkey: me.public,
      nsec: me.nsec,
      tokenAuth: tokenAuth,
      signedOut: signedOut,
      pushNotificationsEnabled: true,
      pushInboxToken: inboxToken ?? token,
      pushSubscriptionState: KuzzPushLeaseSubscriptionState.desired(
        desired: [subscription],
      ),
      addedAt: DateTime.fromMillisecondsSinceEpoch(0),
    );
  }

  /// A relay answering each `/query` with [answer] (filters → rows or a
  /// status code); records every filter list it got.
  ({http.Client client, List<List<dynamic>> requests}) relay(
    Object Function(List<dynamic> filters) answer,
  ) {
    final requests = <List<dynamic>>[];
    final client = MockClient((request) async {
      expect(request.url.toString(), 'https://relay.example/query');
      expect(request.headers['Authorization'], 'Nostr test');
      final filters = jsonDecode(request.body) as List<dynamic>;
      requests.add(filters);
      final result = answer(filters);
      if (result is int) return http.Response('no', result);
      return http.Response(jsonEncode(result), 200);
    });
    return (client: client, requests: requests);
  }

  AndroidPushWake wake(
    http.Client client, {
    List<Community> Function()? communities,
    String? activeId = 'c1',
    bool restricted = false,
    bool inbox = true,
  }) {
    var list = communities ?? () => [community()];
    return AndroidPushWake(
      client: client,
      loadCommunities: () async => list(),
      loadActiveCommunityId: () async => activeId,
      inbox: inbox ? PushInboxWriter(root.path, now: () => now) : null,
      isAgeRestricted: () async => restricted,
      sign:
          ({
            required String method,
            required String url,
            required List<int> bodyBytes,
            required String? nsec,
          }) async {
            expect(nsec, me.nsec);
            return 'Nostr test';
          },
      now: () => now,
    );
  }

  bool isProfileQuery(List<dynamic> filters) =>
      (filters.single as Map)['kinds'].toString() == '[0]';

  List<Map<String, dynamic>> inboxFiles() {
    final folder = Directory('${root.path}/$token');
    if (!folder.existsSync()) return [];
    return [
      for (final file in folder.listSync().whereType<File>())
        if (file.path.endsWith('.json'))
          jsonDecode(file.readAsStringSync()) as Map<String, dynamic>,
    ];
  }

  String? cursor() {
    final file = File('${root.path}/$token/cursor');
    return file.existsSync() ? file.readAsStringSync() : null;
  }

  test('a failed fetch shows the generic text and keeps nothing', () async {
    final mock = relay((_) => 503);
    final notice = await wake(mock.client).run();
    expect(notice!.generic, isTrue);
    expect(mock.requests, hasLength(2), reason: 'inbox fetch, then fallback');
    expect(inboxFiles(), isEmpty);
    expect(cursor(), isNull);
  });

  test('an empty answer shows nothing and leaves the cursor', () async {
    File('${root.path}/$token/cursor')
      ..createSync(recursive: true)
      ..writeAsStringSync('${nowSeconds - 100}');
    final mock = relay((_) => <Object>[]);
    final notice = await wake(mock.client).run();
    expect(notice, isNull);
    expect(inboxFiles(), isEmpty);
    expect(cursor(), '${nowSeconds - 100}');
    final bounded = mock.requests.first.single as Map;
    expect(
      bounded['since'],
      nowSeconds - 100 - PushInboxCursor.lookbackSeconds,
    );
    expect(bounded['limit'], AndroidPushWake.inboxFetchLimit);
  });

  test(
    'a match is kept, moves the cursor and names the sender (active community)',
    () async {
      final message = signed(
        alice,
        at: nowSeconds - 5,
        content: 'see ```x``` now',
      );
      final profile = signed(
        alice,
        kind: 0,
        at: nowSeconds - 999,
        content: jsonEncode({'display_name': 'Alice'}),
      );
      final mine = signed(me, at: nowSeconds - 1);
      final mock = relay(
        (filters) => isProfileQuery(filters) ? [profile] : [message, mine],
      );
      final notice = await wake(mock.client).run();
      expect(notice!.generic, isFalse);
      expect(notice.title, 'Alice');
      expect(notice.subtitle, 'Team');
      expect(notice.body, 'see [code] now');
      expect(notice.eventId, message['id']);
      expect(notice.communityId, 'c1');
      expect(notice.channelId, '11111111-1111-4111-8111-111111111111');
      final file = inboxFiles().single;
      expect(file['account'], me.public);
      expect(file['relayUrl'], 'wss://relay.example');
      expect(file['fetchedAt'], nowSeconds);
      expect([
        for (final e in file['events'] as List) (e as Map)['id'],
      ], unorderedEquals([message['id'], mine['id'], profile['id']]));
      expect(cursor(), '${nowSeconds - 1}');
    },
  );

  test('N10: the sender profile is not kept for another community', () async {
    final message = signed(alice, at: nowSeconds - 5);
    final profile = signed(
      alice,
      kind: 0,
      at: nowSeconds - 999,
      content: jsonEncode({'name': 'Alice'}),
    );
    final mock = relay(
      (filters) => isProfileQuery(filters) ? [profile] : [message],
    );
    final notice = await wake(mock.client, activeId: 'other').run();
    expect(notice!.title, 'Alice');
    expect(
      [for (final e in inboxFiles().single['events'] as List) (e as Map)['id']],
      [message['id']],
    );
  });

  test('without a profile the community name is the title', () async {
    final message = signed(alice, at: nowSeconds - 5);
    final mock = relay(
      (filters) => isProfileQuery(filters) ? <Object>[] : [message],
    );
    final notice = await wake(mock.client).run();
    expect(notice!.title, 'Team');
    expect(notice.subtitle, isNull);
  });

  group('room titles', () {
    final relayKeys = nostr.Keys.generate();
    final bob = nostr.Keys.generate();
    const roomId = '11111111-1111-4111-8111-111111111111';

    Map<String, dynamic> metadata(List<List<String>> tags) => signed(
      relayKeys,
      kind: 39000,
      at: nowSeconds - 100,
      extraTags: [
        ['d', roomId],
        ...tags,
      ],
    );

    Future<AndroidPushNotice> notice(
      Map<String, dynamic> message, {
      List<Map<String, dynamic>> rows = const [],
      Map<String, dynamic>? roster,
    }) async {
      final profile = signed(
        alice,
        kind: 0,
        at: nowSeconds - 50,
        content: jsonEncode({'name': 'Alice'}),
      );
      final bobProfile = signed(
        bob,
        kind: 0,
        at: nowSeconds - 50,
        content: jsonEncode({'name': 'Bob'}),
      );
      final mock = relay((filters) {
        final kinds = (filters.first as Map)['kinds'].toString();
        if (kinds == '[0]') {
          return (filters.first as Map)['authors'].contains(alice.public)
              ? [profile]
              : [bobProfile];
        }
        if (kinds == '[39000]') return rows;
        if (kinds == '[39002]') return [?roster];
        return [message];
      });
      return (await wake(mock.client).run())!;
    }

    test('a group is titled by its name', () async {
      final result = await notice(
        signed(alice, at: nowSeconds - 5),
        rows: [
          metadata([
            ['name', 'Kuzz'],
            ['t', 'stream'],
          ]),
        ],
      );
      expect(result.conversationTitle, 'Kuzz');
      expect(result.title, 'Alice');
      expect(result.threadHeadId, isNull);
    });

    test('a 1:1 DM has no room title', () async {
      final result = await notice(
        signed(alice, at: nowSeconds - 5),
        rows: [
          metadata([
            ['name', 'DM'],
            ['t', 'dm'],
            ['p', alice.public],
          ]),
        ],
      );
      expect(result.conversationTitle, isNull);
      expect(result.title, 'Alice');
    });

    test('an unknown room falls back to the sender', () async {
      final result = await notice(signed(alice, at: nowSeconds - 5));
      expect(result.conversationTitle, isNull);
      expect(result.title, 'Alice');
    });

    test('a group DM is titled by its other members', () async {
      final result = await notice(
        signed(alice, at: nowSeconds - 5),
        rows: [
          metadata([
            ['name', 'Group DM (3)'],
            ['t', 'dm'],
            ['p', alice.public],
            ['p', bob.public],
          ]),
        ],
      );
      expect(result.conversationTitle, 'Alice, Bob');
    });

    test('an unnamed group chat reads its roster', () async {
      final result = await notice(
        signed(alice, at: nowSeconds - 5),
        rows: [
          metadata([
            ['t', 'stream'],
            ['group_chat'],
          ]),
        ],
        roster: signed(
          relayKeys,
          kind: 39002,
          at: nowSeconds - 100,
          extraTags: [
            ['d', roomId],
            ['p', bob.public, '', 'member'],
            ['p', alice.public, '', 'member'],
          ],
        ),
      );
      expect(result.conversationTitle, 'Bob, Alice');
    });

    test('a nested reply is titled by the thread head', () async {
      final head = signed(alice, at: nowSeconds - 90, content: 'Launch plan');
      final parent = 'b' * 64;
      final message = signed(
        alice,
        at: nowSeconds - 5,
        extraTags: [
          ['e', head['id'] as String, '', 'root'],
          ['e', parent, '', 'reply'],
        ],
      );
      final group = metadata([
        ['name', 'Kuzz'],
      ]);
      final unnamed = await notice(message, rows: [group, head]);
      expect(unnamed.conversationTitle, 'Kuzz > Launch plan');
      expect(unnamed.threadHeadId, head['id']);
      expect(unnamed.threadRootId, parent);

      final hiddenRoot = await notice(message, rows: [group]);
      expect(hiddenRoot.conversationTitle, 'Kuzz > hello');

      final named = await notice(
        message,
        rows: [
          group,
          head,
          signed(
            alice,
            kind: 40009,
            at: nowSeconds - 20,
            content: 'Launch',
            extraTags: [
              ['e', head['id'] as String],
            ],
          ),
        ],
      );
      expect(named.conversationTitle, 'Kuzz > Launch');
    });

    test('a huge thread root does not lose the room name', () async {
      final mock = relay((filters) {
        final kinds = (filters.first as Map)['kinds'].toString();
        if (kinds == '[39000]') {
          return [
            metadata([
              ['name', 'Kuzz'],
            ]),
            signed(alice, at: nowSeconds - 90, content: 'x' * (300 * 1024)),
          ];
        }
        return kinds == '[0]'
            ? <Object>[]
            : [signed(alice, at: nowSeconds - 5)];
      });
      final result = await wake(mock.client).run();
      expect(result!.conversationTitle, 'Kuzz');
    });

    test('notice arguments carry the room title and thread head', () {
      final head = 'a' * 64;
      final arguments = AndroidPushNotice.message(
        title: 'Alice',
        body: 'hi',
        conversationTitle: 'Kuzz > Plan',
        eventId: 'e',
        communityId: 'c',
        channelId: roomId,
        threadRootId: head,
        threadHeadId: head,
      ).toChannelArguments(lookupAppLocalizations(const Locale('en')));
      expect(arguments['conversationTitle'], 'Kuzz > Plan');
      expect(arguments['threadHeadId'], head);
    });
  });

  test(
    'a thread reply notice opens its thread, a broadcast the room',
    () async {
      final head = 'a' * 64;
      final parent = 'b' * 64;
      Future<AndroidPushNotice?> noticeFor(List<List<String>> tags) {
        final message = signed(alice, at: nowSeconds - 5, extraTags: tags);
        return wake(
          relay(
            (filters) => isProfileQuery(filters) ? <Object>[] : [message],
          ).client,
        ).run();
      }

      expect((await noticeFor([]))!.threadRootId, isNull);
      expect((await noticeFor([]))!.createdAt, nowSeconds - 5);
      expect(
        (await noticeFor([
          ['e', head, '', 'reply'],
        ]))!.threadRootId,
        head,
      );
      expect(
        (await noticeFor([
          ['e', head, '', 'root'],
          ['e', parent, '', 'reply'],
        ]))!.threadRootId,
        parent,
      );
      expect(
        (await noticeFor([
          ['e', head, '', 'reply'],
          ['broadcast', '1'],
        ]))!.threadRootId,
        isNull,
      );
    },
  );

  test('forged and unmatched rows are neither shown nor kept', () async {
    final forged = {...signed(alice, at: nowSeconds - 5), 'content': 'changed'};
    final other = signed(alice, at: nowSeconds - 4, kind: 1);
    final mock = relay((_) => [forged, other]);
    final notice = await wake(mock.client).run();
    expect(notice, isNull);
    expect(inboxFiles(), isEmpty);
    // `other` verified, so the cursor still advances to it.
    expect(cursor(), '${nowSeconds - 4}');
  });

  test(
    'the fallback decides the notification but never moves the cursor',
    () async {
      final old = signed(alice, at: nowSeconds - 400000);
      final mock = relay(
        (filters) => isProfileQuery(filters)
            ? <Object>[]
            : (filters.first as Map).containsKey('since')
            ? <Object>[]
            : [old],
      );
      final notice = await wake(mock.client).run();
      expect(notice!.eventId, old['id']);
      expect((mock.requests[1].first as Map)['limit'], 10);
      expect((mock.requests[1].first as Map).containsKey('since'), isFalse);
      expect(inboxFiles().single['events'], hasLength(1));
      expect(cursor(), isNull);
    },
  );

  test('age-restricted: generic text, no fetch, nothing kept', () async {
    final mock = relay((_) => [signed(alice, at: nowSeconds)]);
    final notice = await wake(mock.client, restricted: true).run();
    expect(notice!.generic, isTrue);
    expect(mock.requests, isEmpty);
    expect(inboxFiles(), isEmpty);
  });

  test('a token removed during the run is not written', () async {
    var calls = 0;
    final message = signed(alice, at: nowSeconds - 5);
    final mock = relay(
      (filters) => isProfileQuery(filters) ? <Object>[] : [message],
    );
    final notice = await wake(
      mock.client,
      communities: () =>
          calls++ == 0 ? [community()] : [community(inboxToken: 'ef' * 16)],
    ).run();
    expect(notice!.eventId, message['id']);
    expect(inboxFiles(), isEmpty);
    expect(cursor(), isNull);
  });

  test('token and signed-out communities are not fetched', () async {
    final mock = relay((_) => 503);
    final notice = await wake(
      mock.client,
      communities: () => [
        community(tokenAuth: true),
        community(id: 'c2', signedOut: true),
      ],
    ).run();
    expect(notice, isNull);
    expect(mock.requests, isEmpty);
  });

  test('a slow relay is cut off at the deadline', () async {
    final client = MockClient((request) async {
      await Future<void>.delayed(const Duration(milliseconds: 300));
      return http.Response('[]', 200);
    });
    final notice = await AndroidPushWake(
      client: client,
      loadCommunities: () async => [community()],
      loadActiveCommunityId: () async => 'c1',
      inbox: PushInboxWriter(root.path),
      isAgeRestricted: () async => false,
      sign:
          ({
            required String method,
            required String url,
            required List<int> bodyBytes,
            required String? nsec,
          }) async => 'Nostr test',
      boundedDeadline: const Duration(milliseconds: 50),
      fallbackDeadline: const Duration(milliseconds: 100),
    ).run();
    expect(notice!.generic, isTrue);
  });

  test('an answer over the byte budget counts as failed', () async {
    final mock = relay(
      (_) => [signed(alice, at: nowSeconds, content: 'x' * 400)],
    );
    final notice = await AndroidPushWake(
      client: mock.client,
      loadCommunities: () async => [community()],
      loadActiveCommunityId: () async => 'c1',
      inbox: PushInboxWriter(root.path),
      isAgeRestricted: () async => false,
      sign:
          ({
            required String method,
            required String url,
            required List<int> bodyBytes,
            required String? nsec,
          }) async => 'Nostr test',
      byteBudget: 100,
    ).run();
    expect(notice!.generic, isTrue);
    expect(inboxFiles(), isEmpty);
  });

  test('generic and message notices carry localized channel text', () {
    final strings = lookupAppLocalizations(const Locale('en'));
    final generic = const AndroidPushNotice.generic().toChannelArguments(
      strings,
    );
    expect(generic['body'], 'New message');
    expect(generic['channelName'], 'Messages');
    expect(generic.containsKey('eventId'), isFalse);
    final message = const AndroidPushNotice.message(
      title: 'Alice',
      body: 'hi',
      eventId: 'e',
      communityId: 'c',
      channelId: 'r',
    ).toChannelArguments(strings);
    expect(message['title'], 'Alice');
    expect(message['eventId'], 'e');
    expect(message.containsKey('threadRootId'), isFalse);
    expect(message.containsKey('createdAt'), isFalse);
    expect(
      const AndroidPushNotice.message(
        title: 'Alice',
        body: 'hi',
        eventId: 'e',
        communityId: 'c',
        channelId: 'r',
        createdAt: 42,
      ).toChannelArguments(strings)['createdAt'],
      42,
    );
    final reply = AndroidPushNotice.message(
      title: 'Alice',
      body: 'hi',
      eventId: 'e',
      communityId: 'c',
      channelId: 'r',
      threadRootId: 'a' * 64,
    ).toChannelArguments(strings);
    expect(reply['threadRootId'], 'a' * 64);
    final malformed = const AndroidPushNotice.message(
      title: 'Alice',
      body: 'hi',
      eventId: 'e',
      communityId: 'c',
      channelId: 'r',
      threadRootId: 'not-an-id',
    ).toChannelArguments(strings);
    expect(malformed.containsKey('threadRootId'), isFalse);
    expect(malformed['channelId'], 'r');
    final korean = lookupAppLocalizations(const Locale('ko'));
    expect(
      const AndroidPushNotice.generic().toChannelArguments(korean)['body'],
      korean.pushAndroidGenericNotification,
    );
  });

  test('previewBody follows the iPhone rules', () {
    expect(
      previewBody('a `b` [c](https://d) https://e.example/f'),
      'a b c [link]',
    );
    expect(previewBody('  x\n\n y '), 'x y');
    final long = previewBody('w ' * 200);
    expect(long.runes.length, lessThanOrEqualTo(178));
    expect(long.endsWith('…'), isTrue);
  });

  test('queryUrl maps the relay socket URL to its /query', () {
    expect(queryUrl('wss://relay.example'), 'https://relay.example/query');
    expect(queryUrl('ws://localhost:3000'), 'http://localhost:3000/query');
  });
}
