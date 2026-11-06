import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/leave_with_agents.dart';
import 'package:kuzz/features/channels/members_sheet.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

const _channelId = 'channel-1';
const _me = 'me';
final _mine = 'a' * 64;
final _theirs = 'b' * 64;
final _pairedAsMember = 'c' * 64;

final _channel = Channel(
  id: _channelId,
  name: 'general',
  channelType: 'stream',
  visibility: 'private',
  description: '',
  createdBy: 'someone',
  createdAt: DateTime(2025),
  memberCount: 3,
  isMember: true,
);

ChannelMember _member(String pubkey, String role) => ChannelMember(
  pubkey: pubkey,
  role: role,
  joinedAt: DateTime(2025),
  displayName: {_mine: 'Mine', _pairedAsMember: 'Paired'}[pubkey] ?? 'Theirs',
);

void main() {
  Future<void> openAgentMenu(WidgetTester tester, String name) async {
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          channelMembersProvider(_channelId).overrideWith(
            (ref) async => [
              _member(_me, 'member'),
              _member(_mine, 'bot'),
              _member(_theirs, 'bot'),
              _member(_pairedAsMember, 'member'),
            ],
          ),
          myAgentsInChannelProvider(
            _channelId,
          ).overrideWithValue([_mine, _pairedAsMember]),
        ],
        child: MaterialApp(
          theme: AppTheme.light(),
          home: Scaffold(
            body: MembersSheet(channel: _channel, currentPubkey: _me),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(
      find.descendant(
        of: find.ancestor(
          of: find.textContaining(name),
          matching: find.byType(ListTile),
        ),
        matching: find.byIcon(LucideIcons.ellipsis),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('a member may remove their own agent without a channel role', (
    tester,
  ) async {
    await openAgentMenu(tester, 'Mine');
    expect(find.text('Remove from group'), findsOneWidget);
  });

  testWidgets('an own agent listed with people gets Remove, not the role', (
    tester,
  ) async {
    await openAgentMenu(tester, 'Paired');
    expect(find.text('Remove from group'), findsOneWidget);
    expect(find.text('Role'), findsNothing);
  });

  testWidgets("a member may not remove someone else's agent", (tester) async {
    await openAgentMenu(tester, 'Theirs');
    expect(find.text('Remove from group'), findsNothing);
  });
}
