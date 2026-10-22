import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/groups/group_invitations.dart';
import 'package:kuzz/features/groups/groups_api.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/theme/theme.dart';

const _inviter =
    'a11ce00000000000000000000000000000000000000000000000000000000000';

GroupInvitation _invitation(String id, String name, {int members = 4}) =>
    GroupInvitation(
      id: id,
      group: GroupSummary(
        id: 'group-$id',
        name: name,
        description: '',
        channelType: 'stream',
        memberCount: members,
        isMember: false,
        visibility: 'private',
      ),
      inviter: _inviter,
      createdAt: DateTime.utc(2026, 10, 20),
    );

class _FakeGroupsApi extends Fake implements GroupsApi {
  _FakeGroupsApi(this.list);

  List<GroupInvitation> list;
  Object? invitationsError;
  Object? acceptError;
  Object? declineError;
  final declined = <String>[];
  int invitationsCalls = 0;

  @override
  Future<List<GroupInvitation>> invitations() async {
    invitationsCalls++;
    if (invitationsError case final error?) throw error;
    return list;
  }

  @override
  Future<String> acceptInvitation(String id) async {
    if (acceptError case final error?) throw error;
    return 'group-$id';
  }

  @override
  Future<void> declineInvitation(String id) async {
    if (declineError case final error?) throw error;
    declined.add(id);
  }
}

class _FakeUserCacheNotifier extends UserCacheNotifier {
  @override
  Map<String, UserProfile> build() => const {
    _inviter: UserProfile(pubkey: _inviter, displayName: 'Alice'),
  };

  @override
  Future<bool> preload(List<String> pubkeys) async => true;
}

void main() {
  group('GroupInvitationsNotifier', () {
    late _FakeGroupsApi api;
    late ProviderContainer container;

    setUp(() {
      api = _FakeGroupsApi([
        _invitation('1', 'Book club'),
        _invitation('2', 'Hiking'),
      ]);
      container = ProviderContainer(
        overrides: [groupsApiProvider.overrideWithValue(api)],
      );
      addTearDown(container.dispose);
    });

    test('loads invitations', () async {
      final list = await container.read(groupInvitationsProvider.future);
      expect(list.map((i) => i.id), ['1', '2']);
    });

    test('a refresh failure keeps the list shown', () async {
      await container.read(groupInvitationsProvider.future);
      api.invitationsError = const GroupsApiException(500, 'boom');
      await container.read(groupInvitationsProvider.notifier).refresh();
      final state = container.read(groupInvitationsProvider);
      expect(state.hasError, isFalse);
      expect(state.value!.map((i) => i.id), ['1', '2']);
    });

    test('accept removes the invitation', () async {
      await container.read(groupInvitationsProvider.future);
      final notifier = container.read(groupInvitationsProvider.notifier);
      expect(await notifier.accept(_invitation('1', 'Book club')), 'group-1');
      expect(container.read(groupInvitationsProvider).value!.map((i) => i.id), [
        '2',
      ]);
    });

    test('an invitation that is gone (409/404) is removed', () async {
      await container.read(groupInvitationsProvider.future);
      final notifier = container.read(groupInvitationsProvider.notifier);
      api.acceptError = const GroupsApiException(409, 'already used');
      await expectLater(
        notifier.accept(_invitation('1', 'Book club')),
        throwsA(isA<GroupsApiException>()),
      );
      api.acceptError = const GroupsApiException(404, 'not found');
      await expectLater(
        notifier.accept(_invitation('2', 'Hiking')),
        throwsA(isA<GroupsApiException>()),
      );
      expect(container.read(groupInvitationsProvider).value, isEmpty);
    });

    test('another accept failure keeps the invitation', () async {
      await container.read(groupInvitationsProvider.future);
      api.acceptError = const GroupsApiException(500, 'boom');
      await expectLater(
        container
            .read(groupInvitationsProvider.notifier)
            .accept(_invitation('1', 'Book club')),
        throwsA(isA<GroupsApiException>()),
      );
      expect(container.read(groupInvitationsProvider).value, hasLength(2));
    });

    test('decline removes it, also when it is already gone', () async {
      await container.read(groupInvitationsProvider.future);
      final notifier = container.read(groupInvitationsProvider.notifier);
      await notifier.decline(_invitation('1', 'Book club'));
      api.declineError = const GroupsApiException(404, 'not found');
      await notifier.decline(_invitation('2', 'Hiking'));
      expect(api.declined, ['1']);
      expect(container.read(groupInvitationsProvider).value, isEmpty);
    });
  });

  group('invitations UI', () {
    Future<_FakeGroupsApi> pump(
      WidgetTester tester,
      Widget child,
      List<GroupInvitation> list,
    ) async {
      final api = _FakeGroupsApi(list);
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            groupsApiProvider.overrideWithValue(api),
            userCacheProvider.overrideWith(_FakeUserCacheNotifier.new),
          ],
          child: MaterialApp(
            theme: AppTheme.light(),
            home: Scaffold(body: child),
          ),
        ),
      );
      await tester.pumpAndSettle();
      return api;
    }

    testWidgets('row shows the count and is hidden with none', (tester) async {
      await pump(tester, const GroupInvitationsRow(), [
        _invitation('1', 'Book club'),
        _invitation('2', 'Hiking'),
      ]);
      expect(find.text('Invitations · 2'), findsOneWidget);

      await pump(tester, const GroupInvitationsRow(), const []);
      expect(find.byKey(const Key('chats-invitations-row')), findsNothing);
    });

    testWidgets('page lists invitations with who invited', (tester) async {
      await pump(tester, const GroupInvitationsPage(), [
        _invitation('1', 'Book club', members: 1),
        _invitation('2', 'Hiking', members: 12),
      ]);
      expect(find.text('Invitations'), findsOneWidget);
      expect(find.text('Book club'), findsOneWidget);
      expect(find.text('Alice invited you · 1 member'), findsOneWidget);
      expect(find.text('Alice invited you · 12 members'), findsOneWidget);
      expect(find.textContaining('#'), findsNothing);
    });

    testWidgets('page shows the empty state', (tester) async {
      await pump(tester, const GroupInvitationsPage(), const []);
      expect(find.byKey(const ValueKey('invitations-empty')), findsOneWidget);
      expect(find.text('No invitations'), findsOneWidget);
    });

    testWidgets('page offers a retry when loading fails', (tester) async {
      final api = _FakeGroupsApi(const [])
        ..invitationsError = const GroupsApiException(500, 'boom');
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            groupsApiProvider.overrideWithValue(api),
            userCacheProvider.overrideWith(_FakeUserCacheNotifier.new),
          ],
          child: MaterialApp(
            theme: AppTheme.light(),
            home: const GroupInvitationsPage(),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Try again'), findsOneWidget);

      api
        ..invitationsError = null
        ..list = [_invitation('1', 'Book club')];
      await tester.tap(find.text('Try again'));
      await tester.pumpAndSettle();
      expect(find.text('Book club'), findsOneWidget);
    });

    testWidgets('joining a used invitation says it is no longer valid', (
      tester,
    ) async {
      final api = await pump(tester, const GroupInvitationsPage(), [
        _invitation('1', 'Book club'),
      ]);
      api.acceptError = const GroupsApiException(409, 'already used');
      await tester.tap(find.byKey(const ValueKey('invitation-join-1')));
      await tester.pumpAndSettle();
      expect(find.text(invitationNoLongerValidMessage), findsOneWidget);
      expect(find.byKey(const ValueKey('invitations-empty')), findsOneWidget);
    });

    testWidgets('decline removes the row', (tester) async {
      final api = await pump(tester, const GroupInvitationsPage(), [
        _invitation('1', 'Book club'),
        _invitation('2', 'Hiking'),
      ]);
      await tester.tap(find.byKey(const ValueKey('invitation-decline-1')));
      await tester.pumpAndSettle();
      expect(api.declined, ['1']);
      expect(find.text('Book club'), findsNothing);
      expect(find.text('Hiking'), findsOneWidget);
    });
  });

  group('add member replies', () {
    test('classifies relay replies', () {
      expect(
        classifyAddMemberReply(
          'Relay rejected event: invite-sent: they were sent an invitation',
        ),
        AddMemberReply.invited,
      );
      expect(
        classifyAddMemberReply('invite-sent: they were sent an invitation'),
        AddMemberReply.invited,
      );
      expect(
        classifyAddMemberReply(
          'Exception: Relay rejected event: rate-limited: too many invitations, try later',
        ),
        AddMemberReply.rateLimited,
      );
      expect(
        classifyAddMemberReply('restricted: not allowed'),
        AddMemberReply.other,
      );
    });

    test('messages', () {
      expect(
        invitationSentMessage(['Alice', 'Bob']),
        'Invitation sent to Alice, Bob',
      );
      expect(
        AddMembersException({
          'a' * 64: tooManyInvitationsMessage,
          'b' * 64: tooManyInvitationsMessage,
        }).message,
        'Too many invitations today',
      );
      expect(
        AddMembersException({
          'a' * 64: tooManyInvitationsMessage,
          'b' * 64: 'restricted: nope',
        }).message,
        contains('restricted: nope'),
      );
    });
  });
}
