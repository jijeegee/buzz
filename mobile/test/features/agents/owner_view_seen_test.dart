import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/agents/owner_view/owner_view_seen.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _chat = '11111111-1111-4111-8111-111111111111';
final _agent = 'b' * 64;
final _bob = 'c' * 64;
final _root = 'e' * 64;

ChatListRow _row(int at, {bool thread = false, String? author}) => ChatListRow(
  isThread: thread,
  key: thread ? _root : _chat,
  at: at,
  atUs: at * 1000000,
  channelId: _chat,
  channelType: 'dm',
  channelName: 'DM',
  rootId: thread ? _root : null,
  unread: 4,
  latest: NostrEvent(
    id: 'm$at',
    pubkey: author ?? _bob,
    createdAt: at,
    kind: 9,
    tags: const [],
    content: 'hi',
  ),
);

bool _dot(List<ChatListRow> rows, OwnerViewSeen? seen) =>
    withSeenDots(rows, _agent, seen).single.unreadMark;

void main() {
  test('nothing has a dot before the first look', () {
    expect(_dot([_row(500)], null), isFalse);
  });

  test('activity after the baseline gets a dot, older does not', () {
    const seen = OwnerViewSeen(baseline: 100);
    expect(_dot([_row(90)], seen), isFalse);
    expect(_dot([_row(100)], seen), isFalse);
    expect(_dot([_row(101)], seen), isTrue);
    // Counts never show here.
    expect(withSeenDots([_row(101)], _agent, seen).single.unread, 0);
  });

  test('the agent\'s own latest message is no dot', () {
    const seen = OwnerViewSeen(baseline: 100);
    expect(_dot([_row(200, author: _agent)], seen), isFalse);
  });

  test('opened up to its latest clears the dot until newer activity', () {
    final seen = const OwnerViewSeen(baseline: 100).markSeen(_chat, 200);
    expect(_dot([_row(200)], seen), isFalse);
    expect(_dot([_row(201)], seen), isTrue);
    expect(seen.markSeen(_chat, 150), same(seen));
  });

  test('a thread is seen apart from its chat', () {
    final seen = const OwnerViewSeen(baseline: 100).markSeen(_chat, 200);
    expect(_dot([_row(200, thread: true)], seen), isTrue);
    final both = seen.markSeen(chatRowContextId(_row(200, thread: true)), 200);
    expect(_dot([_row(200, thread: true)], both), isFalse);
    expect(chatRowContextId(_row(1, thread: true)), 'thread:$_root');
  });

  test('saves at most the newest contexts', () {
    final seen = OwnerViewSeen(
      baseline: 7,
      seen: {for (var i = 0; i < 5; i++) 'c$i': i},
    );
    final decoded = OwnerViewSeen.decode(seen.encode(maxContexts: 3))!;
    expect(decoded.baseline, 7);
    expect(decoded.seen.keys, unorderedEquals(['c2', 'c3', 'c4']));
    expect(OwnerViewSeen.decode('nope'), isNull);
    expect(OwnerViewSeen.decode('{"seen":{}}'), isNull);
  });

  test('the record persists per account and agent', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    ProviderContainer container(String me) => ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        currentPubkeyProvider.overrideWithValue(me),
      ],
    );

    final first = container('a' * 64);
    final notifier = first.read(ownerViewSeenProvider(_agent).notifier);
    notifier.ensureBaseline(now: DateTime.fromMillisecondsSinceEpoch(9000));
    notifier.ensureBaseline(now: DateTime.fromMillisecondsSinceEpoch(99000));
    notifier.markSeen(_chat, 50);
    first.dispose();

    final again = container('a' * 64);
    final record = again.read(ownerViewSeenProvider(_agent))!;
    expect(record.baseline, 9);
    expect(record.seen, {_chat: 50});
    again.dispose();

    final other = container('d' * 64);
    expect(other.read(ownerViewSeenProvider(_agent)), isNull);
    other.dispose();
  });
}
