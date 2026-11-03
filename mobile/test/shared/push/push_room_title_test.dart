import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/push/push_room_title.dart';
import 'package:kuzz/shared/push/push_thread_target.dart';

void main() {
  PushRoom room(List<List<String>> tags) =>
      PushRoom.fromMetadata({'kind': 39000, 'tags': tags})!;
  final me = 'a' * 64;
  final bob = 'b' * 64;
  final cy = 'c' * 64;

  String? title(PushRoom room, {String members = '', String? thread}) =>
      pushConversationTitle(
        room: room,
        memberTitle: members,
        sender: 'Bob',
        threadTitle: thread,
      );

  test('a 1:1 DM is titled by the sender alone, its thread by both', () {
    final dm = room([
      ['t', 'dm'],
      ['name', 'DM'],
      ['p', me],
      ['p', bob],
    ]);
    expect(dm.isOneToOne, isTrue);
    expect(dm.namedByMembers, isFalse);
    expect(title(dm), isNull);
    expect(title(dm, thread: 'Plan'), 'Bob > Plan');
  });

  test('a group is titled by its name, its thread by group > thread', () {
    final group = room([
      ['t', 'stream'],
      ['name', ' Kuzz '],
    ]);
    expect(title(group), 'Kuzz');
    expect(title(group, thread: 'Plan'), 'Kuzz > Plan');
  });

  test('a blank group name leaves the sender', () {
    expect(
      title(
        room([
          ['name', ''],
        ]),
      ),
      isNull,
    );
  });

  test('group DMs and unnamed group chats are titled by their members', () {
    final groupDm = room([
      ['t', 'dm'],
      ['name', 'Group DM (3)'],
      ['p', me],
      ['p', bob],
      ['p', cy],
    ]);
    expect(groupDm.namedByMembers, isTrue);
    expect(title(groupDm, members: 'Bob, Cy'), 'Bob, Cy');

    final chat = room([
      ['t', 'stream'],
      ['name', ''],
      ['group_chat'],
    ]);
    expect(chat.isGroupChat, isTrue);
    expect(chat.namedByMembers, isTrue);
    expect(title(chat, members: 'Bob'), 'Bob');
    expect(title(chat), isNull);

    final named = room([
      ['name', 'Crew'],
      ['group_chat'],
    ]);
    expect(named.namedByMembers, isFalse);
    expect(title(named), 'Crew');
  });

  test('member titles name three people, then count', () {
    final others = pushOtherMembers([me, bob, bob.toUpperCase(), cy], me);
    expect(others, [bob, cy]);
    expect(pushMemberTitle(others, {bob: 'Bob', cy: 'Cy'}), 'Bob, Cy');
    expect(
      pushMemberTitle(['1', '2', '3', '4', '5'].map((c) => c * 64).toList(), {
        for (final c in '12345'.split('')) c * 64: 'P$c',
      }),
      'P1, P2, P3 +2',
    );
    expect(pushMemberTitle(const [], const {}), isEmpty);
  });

  test('a thread title is its name, the root line, else the message', () {
    expect(
      pushThreadTitle(threadName: ' Plan ', rootContent: 'x', fallback: 'y'),
      'Plan',
    );
    expect(
      pushThreadTitle(
        threadName: '',
        rootContent: '\n first \nx',
        fallback: 'y',
      ),
      'first',
    );
    expect(pushThreadTitle(fallback: 'y'), 'y');
    expect(
      pushThreadTitle(rootContent: 'z' * 90, fallback: 'y'),
      '${'z' * 80}…',
    );
  });

  test('the thread head is the root tag, else the reply parent', () {
    final root = 'a' * 64;
    final parent = 'b' * 64;
    expect(
      pushThreadHeadId([
        ['e', root, '', 'root'],
        ['e', parent, '', 'reply'],
      ]),
      root,
    );
    expect(
      pushThreadHeadId([
        ['e', parent, '', 'reply'],
      ]),
      parent,
    );
    expect(
      pushThreadHeadId([
        ['e', root, '', 'root'],
        ['e', parent, '', 'reply'],
        ['broadcast', '1'],
      ]),
      isNull,
    );
    expect(
      pushThreadHeadId([
        ['h', 'room'],
      ]),
      isNull,
    );
  });
}
