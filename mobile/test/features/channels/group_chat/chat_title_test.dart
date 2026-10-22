import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/group_avatar.dart';
import 'package:kuzz/features/channels/group_chat/chat_grid_avatar.dart';
import 'package:kuzz/features/channels/group_chat/chat_info_sheet.dart';
import 'package:kuzz/features/channels/group_chat/chat_title.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';

final _me = 'a' * 64;
final _ann = 'b' * 64;
final _bob = 'c' * 64;
final _cy = 'd' * 64;
final _dee = 'e' * 64;

Channel _chat({
  String name = '',
  List<String> members = const [],
  int memberCount = 0,
  bool isMember = true,
}) => Channel(
  id: 'chat-1',
  name: name,
  channelType: 'stream',
  visibility: 'private',
  description: '',
  createdBy: '',
  createdAt: DateTime(2026),
  memberCount: memberCount,
  isMember: isMember,
  participantPubkeys: members,
  groupChat: true,
);

void main() {
  group('chat title', () {
    test('first three names then +N', () {
      expect(formatChatMemberNames(['Ann']), 'Ann');
      expect(formatChatMemberNames(['Ann', 'Bob', 'Cy']), 'Ann, Bob, Cy');
      expect(
        formatChatMemberNames(['Ann', 'Bob', 'Cy', 'Dee', 'Eve']),
        'Ann, Bob, Cy +2',
      );
      // A partial list still counts everyone else.
      expect(formatChatMemberNames(['Ann', 'Bob'], total: 10), 'Ann, Bob +8');
      expect(formatChatMemberNames(const []), '');
    });

    test('private name, then shared name, then members, then Empty chat', () {
      expect(
        resolveChatTitle(
          privateName: 'Mine',
          sharedName: 'Trip',
          otherMemberLabels: ['Ann'],
        ),
        'Mine',
      );
      expect(
        resolveChatTitle(
          privateName: '  ',
          sharedName: 'Trip',
          otherMemberLabels: ['Ann'],
        ),
        'Trip',
      );
      expect(
        resolveChatTitle(sharedName: '', otherMemberLabels: ['Ann', 'Bob']),
        'Ann, Bob',
      );
      expect(
        resolveChatTitle(sharedName: '', otherMemberLabels: const []),
        emptyChatTitle,
      );
    });

    test('channelTitle leaves me out and counts the rest', () {
      final chat = _chat(members: [_me, _ann, _bob, _cy, _dee], memberCount: 5);
      final title = channelTitle(chat, currentPubkey: _me);
      expect(title, endsWith(' +1'));
      expect(title.split(', '), hasLength(3));
      expect(
        channelTitle(_chat(members: [_me]), currentPubkey: _me),
        'Empty chat',
      );
      expect(
        channelTitle(
          _chat(name: 'Trip', members: [_me, _ann]),
          currentPubkey: _me,
          privateName: 'Family',
        ),
        'Family',
      );
    });

    test('invitations: not a member yet, every member counts', () {
      // Four shown of six others (I am not a member yet): three named, +3.
      final chat = _chat(
        members: [_ann, _bob, _cy, _dee],
        memberCount: 6,
        isMember: false,
      );
      expect(channelTitle(chat, currentPubkey: _me), endsWith(' +3'));
    });

    test('otherChatMembers lowercases, dedupes and drops me', () {
      expect(
        otherChatMembers([
          _me.toUpperCase(),
          _ann,
          _ann.toUpperCase(),
        ], currentPubkey: _me),
        [_ann],
      );
    });

    test('N people', () {
      expect(chatPeopleLabel(1), '1 person');
      expect(chatPeopleLabel(4), '4 people');
    });
  });

  group('chat grid photo', () {
    test('layouts: single, diagonal, one over two, 2x2', () {
      expect(chatGridLayout(1), hasLength(1));
      expect(chatGridLayout(1).single.size, 1);
      final two = chatGridLayout(2);
      expect(two.first.left, 0);
      expect(two.last.left, greaterThan(0));
      expect(two.last.top, greaterThan(0));
      final three = chatGridLayout(3);
      expect(three.first.top, 0);
      expect(three[1].top, three[2].top);
      expect(three[1].top, greaterThan(0));
      expect(chatGridLayout(4), hasLength(4));
      expect(chatGridLayout(9), hasLength(4));
    });

    Future<void> pump(WidgetTester tester, Channel channel) =>
        tester.pumpWidget(
          ProviderScope(
            overrides: [myPubkeyProvider.overrideWithValue(_me)],
            child: MaterialApp(
              theme: AppTheme.light(),
              home: Scaffold(body: GroupAvatar(channel: channel, size: 40)),
            ),
          ),
        );

    testWidgets('a chat draws its other members, no lock', (tester) async {
      await pump(tester, _chat(members: [_me, _ann, _bob, _cy]));
      expect(find.byType(ChatGridAvatar), findsOneWidget);
      expect(find.byKey(ValueKey('chat-grid-avatar-$_ann')), findsOneWidget);
      expect(find.byKey(ValueKey('chat-grid-avatar-$_cy')), findsOneWidget);
      expect(find.byKey(ValueKey('chat-grid-avatar-$_me')), findsNothing);
      expect(
        find.byKey(const ValueKey('group-avatar-lock-badge')),
        findsNothing,
      );
      expect(tester.getSize(find.byType(ChatGridAvatar)), const Size(40, 40));
    });

    testWidgets('alone in a chat shows me', (tester) async {
      await pump(tester, _chat(members: [_me]));
      expect(find.byKey(ValueKey('chat-grid-avatar-$_me')), findsOneWidget);
    });
  });
}
