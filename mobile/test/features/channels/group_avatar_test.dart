import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/group_avatar.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:kuzz/shared/widgets/conversation_row_parts.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

Channel _channel({String type = 'stream', String visibility = 'open'}) =>
    Channel(
      id: 'group-1',
      name: 'design',
      channelType: type,
      visibility: visibility,
      description: '',
      createdBy: 'abc',
      createdAt: DateTime(2025),
      memberCount: 3,
    );

Future<void> _pump(WidgetTester tester, Channel channel) => tester.pumpWidget(
  MaterialApp(
    theme: AppTheme.light(),
    home: Scaffold(body: GroupAvatar(channel: channel, size: 32)),
  ),
);

void main() {
  testWidgets('a group shows its letter on its colour, never a hash', (
    tester,
  ) async {
    await _pump(tester, _channel());

    expect(find.text('D'), findsOneWidget);
    expect(find.byIcon(LucideIcons.hash), findsNothing);
    expect(find.byKey(const ValueKey('group-avatar-lock-badge')), findsNothing);
    final circle = tester.widget<Container>(
      find.ancestor(of: find.text('D'), matching: find.byType(Container)).first,
    );
    expect(
      (circle.decoration! as BoxDecoration).color,
      conversationHue('group-1'),
    );
    expect(tester.getSize(find.byType(GroupAvatar)), const Size.square(32));
  });

  testWidgets('a private group adds a lock badge to the letter', (
    tester,
  ) async {
    await _pump(tester, _channel(visibility: 'private'));

    expect(find.text('D'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('group-avatar-lock-badge')),
      findsOneWidget,
    );
    expect(find.byIcon(LucideIcons.lock), findsOneWidget);
  });

  testWidgets('a forum keeps its forum icon', (tester) async {
    await _pump(tester, _channel(type: 'forum'));

    expect(find.byIcon(LucideIcons.messageSquareText), findsOneWidget);
    expect(find.text('D'), findsNothing);
  });
}
