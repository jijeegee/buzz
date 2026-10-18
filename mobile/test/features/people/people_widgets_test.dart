import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/features/people/blocked_message_gate.dart';
import 'package:kuzz/features/people/people_page.dart';
import 'package:kuzz/features/people/person_actions.dart';
import 'package:kuzz/features/people/person_page.dart';
import 'package:kuzz/features/profile/user_profile_sheet.dart';
import 'package:kuzz/features/settings/blocked_users_page.dart';
import 'package:kuzz/features/settings/kuzz_id_page.dart';
import 'package:kuzz/features/settings/people_settings.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/identity_names/identity_names_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:kuzz/shared/widgets/bee_refresh_indicator.dart';

import 'people_fixture.dart';

final dan = 'd' * 64;

void main() {
  late PeopleSession session;
  late FakePeopleApi api;

  setUp(() {
    session = PeopleSession();
    api = FakePeopleApi();
  });

  Future<void> pumpPage(
    WidgetTester tester,
    Widget page, {
    Set<String> favorites = const {},
    List<String> agents = const [],
    bool withAccount = true,
    Map<String, String>? names,
  }) async {
    tester.view.physicalSize = const Size(800, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      ProviderScope(
        overrides: peopleOverrides(
          session: session,
          api: withAccount ? api : null,
          favorites: favorites,
          agents: agents,
          names: names ?? {ann: 'Ann', bob: 'Bob', cat: 'Cat', dan: 'Dan'},
        ),
        child: MaterialApp(theme: AppTheme.light(), home: page),
      ),
    );
    await tester.pumpAndSettle();
  }

  group('People tab', () {
    testWidgets(
      'lists favorites and friends without blocked people or agents',
      (tester) async {
        session.follow(peopleKeys.public, [ann, bob, cat, dan]);
        api.blocked.add(cat);
        await pumpPage(
          tester,
          const PeoplePage(),
          favorites: {bob},
          agents: [dan],
        );

        expect(find.text('Favorites'), findsOneWidget);
        expect(find.byKey(ValueKey('people-favorite-$bob')), findsOneWidget);
        expect(find.text('Friends · 2'), findsOneWidget);
        expect(find.byKey(ValueKey('people-friend-$ann')), findsOneWidget);
        expect(find.byKey(ValueKey('people-friend-$bob')), findsOneWidget);
        expect(find.text('Cat'), findsNothing);
        expect(find.text('Dan'), findsNothing);
      },
    );

    testWidgets('search shows matching friends, then people in my channels', (
      tester,
    ) async {
      session.follow(peopleKeys.public, [ann, bob]);
      api.shared = [
        PersonSummary(pubkey: bob, displayName: 'Bob'),
        PersonSummary(pubkey: dan, displayName: 'Bea', username: 'bea'),
        PersonSummary(pubkey: cat, displayName: 'Bert'),
      ];
      api.blocked.add(cat);
      await pumpPage(tester, const PeoplePage());

      await tester.enterText(find.byKey(const Key('people-search')), 'b');
      await tester.pump();
      expect(find.byKey(ValueKey('people-friend-$bob')), findsOneWidget);
      expect(find.byKey(ValueKey('people-friend-$ann')), findsNothing);
      expect(api.calls.where((c) => c.startsWith('shared')), isEmpty);

      await tester.pump(peopleSearchDebounce);
      await tester.pumpAndSettle();
      expect(api.calls.where((c) => c.startsWith('shared')), ['shared b']);
      expect(find.text('In your channels'), findsOneWidget);
      // Bob is already listed as a friend; Bert is blocked.
      expect(find.byKey(ValueKey('people-shared-$bob')), findsNothing);
      expect(find.byKey(ValueKey('people-shared-$dan')), findsOneWidget);
      expect(find.byKey(ValueKey('people-shared-$cat')), findsNothing);
    });

    testWidgets('Add friend finds one exact match and adds it', (tester) async {
      api.directory['bob'] = PersonSummary(
        pubkey: bob,
        displayName: 'Bob',
        username: 'bob',
      );
      await pumpPage(tester, const PeoplePage());
      expect(find.byKey(const Key('people-friends-empty')), findsOneWidget);

      await tester.tap(find.byKey(const Key('people-add-friend')));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('add-friend-query')),
        'nobody',
      );
      await tester.pump();
      await tester.tap(find.byKey(const Key('add-friend-find')));
      await tester.pumpAndSettle();
      expect(find.text('No one found with that ID or email'), findsOneWidget);

      await tester.enterText(find.byKey(const Key('add-friend-query')), 'bob');
      await tester.pump();
      await tester.tap(find.byKey(const Key('add-friend-find')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('add-friend-result')), findsOneWidget);

      await tester.tap(find.byKey(const Key('add-friend-add')));
      await tester.pumpAndSettle();
      final written = session.published.single;
      expect(written.kind, EventKind.contactList);
      expect(written.tags.map((t) => t[1]), [bob]);
      // Added, with a direct way to start talking.
      expect(
        find.descendant(
          of: find.byKey(const Key('add-friend-result')),
          matching: find.byKey(const Key('add-friend-added')),
        ),
        findsOneWidget,
      );
      expect(find.text('Added'), findsOneWidget);
      expect(find.byKey(const Key('add-friend-open-dm')), findsOneWidget);
      expect(find.byKey(const Key('add-friend-add')), findsNothing);
    });

    testWidgets('pull to refresh shows friends and blocks made elsewhere', (
      tester,
    ) async {
      session.follow(peopleKeys.public, [ann], createdAt: 100);
      await pumpPage(tester, const PeoplePage());
      expect(find.text('Friends · 1'), findsOneWidget);

      // Another device adds Bob and Cat, then blocks Ann.
      session.follow(peopleKeys.public, [ann, bob, cat], createdAt: 200);
      api.blocked.add(ann);
      await tester
          .widget<BeeRefreshIndicator>(find.byType(BeeRefreshIndicator))
          .onRefresh();
      await tester.pumpAndSettle();

      expect(find.text('Friends · 2'), findsOneWidget);
      expect(find.byKey(ValueKey('people-friend-$ann')), findsNothing);
      expect(find.byKey(ValueKey('people-friend-$bob')), findsOneWidget);
      expect(find.byKey(ValueKey('people-friend-$cat')), findsOneWidget);
    });

    testWidgets('too many lookups says so', (tester) async {
      api.lookupError = const AccountApiException('slow', statusCode: 429);
      await pumpPage(tester, const PeoplePage());
      await tester.tap(find.byKey(const Key('people-add-friend')));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('add-friend-query')), 'bob');
      await tester.pump();
      await tester.tap(find.byKey(const Key('add-friend-find')));
      await tester.pumpAndSettle();
      expect(
        find.text('Too many searches. Wait a minute and try again.'),
        findsOneWidget,
      );
    });
  });

  group('person page', () {
    testWidgets('Block confirms, blocks and removes the friend', (
      tester,
    ) async {
      session.follow(peopleKeys.public, [ann, bob]);
      await pumpPage(tester, PersonPage(pubkey: ann));
      expect(find.byKey(const Key('person-name')), findsOneWidget);
      expect(find.text('Remove friend'), findsOneWidget);

      await tester.tap(find.byKey(const Key('person-block')));
      await tester.pumpAndSettle();
      expect(find.text('Block Ann?'), findsOneWidget);
      await tester.tap(find.byKey(const Key('confirm-block')));
      await tester.pumpAndSettle();

      expect(api.blocked, {ann});
      expect(session.published.single.tags.map((t) => t[1]), [bob]);
      expect(find.text('Unblock'), findsOneWidget);
    });

    testWidgets('Remove friend asks first, then confirms', (tester) async {
      session.follow(peopleKeys.public, [ann, bob]);
      await pumpPage(tester, PersonPage(pubkey: ann));

      await tester.tap(find.byKey(const Key('person-friend')));
      await tester.pumpAndSettle();
      expect(find.text('Remove Ann from friends?'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(session.published, isEmpty);
      expect(find.text('Remove friend'), findsOneWidget);

      await tester.tap(find.byKey(const Key('person-friend')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('confirm-remove-friend')));
      await tester.pumpAndSettle();
      expect(session.published.single.tags.map((t) => t[1]), [bob]);
      expect(find.text('Removed Ann from friends'), findsOneWidget);
      expect(find.text('Add friend'), findsOneWidget);
    });

    testWidgets('Add friend confirms with a snack bar', (tester) async {
      await pumpPage(tester, PersonPage(pubkey: ann));
      await tester.tap(find.byKey(const Key('person-friend')));
      await tester.pumpAndSettle();
      expect(session.published.single.tags.map((t) => t[1]), [ann]);
      expect(find.text('Added Ann to friends'), findsOneWidget);
    });

    testWidgets('someone without a loaded name is "this user"', (tester) async {
      final eve = 'e' * 64;
      await pumpPage(tester, PersonPage(pubkey: eve));
      await tester.tap(find.byKey(const Key('person-block')));
      await tester.pumpAndSettle();
      expect(find.text('Block this user?'), findsOneWidget);
      await tester.tap(find.byKey(const Key('confirm-block')));
      await tester.pumpAndSettle();
      expect(find.text('Blocked this user'), findsOneWidget);
    });

    testWidgets('without an account there is no Block, only Report', (
      tester,
    ) async {
      await pumpPage(tester, PersonPage(pubkey: ann), withAccount: false);
      expect(find.byKey(const Key('person-block')), findsNothing);
      expect(find.byKey(const Key('person-report')), findsOneWidget);
    });

    testWidgets('the favorite star toggles', (tester) async {
      await pumpPage(tester, PersonPage(pubkey: ann));
      await tester.tap(find.byKey(const Key('person-favorite')));
      await tester.pump();
      expect(find.byTooltip('Remove from favorites'), findsOneWidget);
    });
  });

  group('profile sheet', () {
    Widget launcher(String pubkey) => Builder(
      builder: (context) => Scaffold(
        body: TextButton(
          onPressed: () => showUserProfileSheet(context, pubkey),
          child: const Text('open'),
        ),
      ),
    );

    testWidgets('has no key to copy', (tester) async {
      await pumpPage(tester, launcher(ann));
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      expect(find.byType(UserProfileSheet), findsOneWidget);
      expect(find.text('Copy public key'), findsNothing);
      expect(find.text('Message'), findsOneWidget);
      await tester.tap(find.byTooltip('Close sheet'));
      await tester.pumpAndSettle();
    });

    testWidgets('Block closes the sheet so its confirmation shows', (
      tester,
    ) async {
      await pumpPage(tester, launcher(ann));
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('profile-sheet-block')));
      await tester.pumpAndSettle();
      expect(find.text('Block Ann?'), findsOneWidget);
      await tester.tap(find.byKey(const Key('confirm-block')));
      await tester.pumpAndSettle();

      expect(api.blocked, {ann});
      expect(find.byType(UserProfileSheet), findsNothing);
      expect(find.text('Blocked Ann'), findsOneWidget);
    });

    testWidgets('Report closes the sheet so its confirmation shows', (
      tester,
    ) async {
      await pumpPage(tester, launcher(ann));
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('profile-sheet-report')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('report-reason-spam')));
      await tester.tap(find.byKey(const Key('report-also-block')));
      await tester.pump();
      await tester.tap(find.byKey(const Key('report-submit')));
      await tester.pumpAndSettle();

      expect(find.byType(UserProfileSheet), findsNothing);
      expect(find.text('Report sent to the moderators'), findsOneWidget);
    });

    testWidgets('a same-name key suffix stays out of the prompts', (
      tester,
    ) async {
      // Two people named Ann: the opener's context tells them apart with a
      // key suffix, which the Block prompt leaves out.
      await pumpPage(
        tester,
        Consumer(
          builder: (context, ref, _) => Scaffold(
            body: TextButton(
              onPressed: () {
                final opened = ref.read(identityNameSourcesProvider).scope([
                  ann,
                  dan,
                ]);
                showUserProfileSheet(
                  context,
                  ann,
                  names: liveIdentityNamesProvider(opened),
                );
              },
              child: const Text('open'),
            ),
          ),
        ),
        names: {ann: 'Ann', dan: 'Ann'},
      );
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      expect(find.textContaining('Ann · '), findsOneWidget);

      await tester.tap(find.byKey(const Key('profile-sheet-block')));
      await tester.pumpAndSettle();
      expect(find.text('Block Ann?'), findsOneWidget);
    });

    testWidgets('an unnamed person is never named by a key fragment', (
      tester,
    ) async {
      final eve = 'e' * 64;
      await pumpPage(tester, launcher(eve));
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('profile-sheet-block')));
      await tester.pumpAndSettle();
      expect(find.text('Block this user?'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      // The heading is neutral too.
      expect(find.text('Kuzz user'), findsOneWidget);
      expect(find.textContaining('npub'), findsNothing);
    });
  });

  group('report sheet', () {
    Widget launcher({String? eventId}) => Builder(
      builder: (context) => Scaffold(
        body: TextButton(
          onPressed: () => showReportSheet(
            context,
            pubkey: ann,
            name: 'Ann',
            eventId: eventId,
          ),
          child: const Text('open'),
        ),
      ),
    );

    testWidgets('reports a message and blocks by default', (tester) async {
      await pumpPage(tester, launcher(eventId: 'e1'));
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      expect(find.text('Report message'), findsOneWidget);
      expect(
        tester
            .widget<CheckboxListTile>(
              find.byKey(const Key('report-also-block')),
            )
            .value,
        isTrue,
      );
      final submit = find.byKey(const Key('report-submit'));
      expect(tester.widget<FilledButton>(submit).onPressed, isNull);

      await tester.tap(find.byKey(const Key('report-reason-spam')));
      await tester.pump();
      await tester.tap(submit);
      await tester.pumpAndSettle();

      final report = session.published.single;
      expect(report.kind, EventKind.report);
      expect(report.tags, [
        ['p', ann],
        ['e', 'e1', 'spam'],
      ]);
      expect(api.blocked, {ann});
      expect(find.text('Report sent. Ann is blocked.'), findsOneWidget);
    });

    testWidgets('a person report without blocking', (tester) async {
      await pumpPage(tester, launcher());
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('report-reason-other')));
      await tester.tap(find.byKey(const Key('report-also-block')));
      await tester.pump();
      await tester.tap(find.byKey(const Key('report-submit')));
      await tester.pumpAndSettle();

      expect(session.published.single.tags, [
        ['p', ann, 'other'],
      ]);
      expect(api.blocked, isEmpty);
      expect(find.text('Report sent to the moderators'), findsOneWidget);
    });

    testWidgets('a refused report stays open with the reason', (tester) async {
      session.publishError = StateError('rejected');
      await pumpPage(tester, launcher());
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('report-reason-spam')));
      await tester.pump();
      await tester.tap(find.byKey(const Key('report-submit')));
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('report-error')), findsOneWidget);
      expect(api.blocked, isEmpty);
    });
  });

  testWidgets('a blocked author\'s message folds until tapped', (tester) async {
    api.blocked.add(ann);
    await pumpPage(
      tester,
      Scaffold(
        body: Column(
          children: [
            BlockedMessageGate(authorPubkey: ann, child: const Text('hidden')),
            BlockedMessageGate(authorPubkey: bob, child: const Text('shown')),
          ],
        ),
      ),
    );
    expect(find.text('hidden'), findsNothing);
    expect(find.text('shown'), findsOneWidget);
    expect(find.text('Message from a blocked user'), findsOneWidget);

    await tester.tap(find.byKey(const Key('blocked-message')));
    await tester.pump();
    expect(find.text('hidden'), findsOneWidget);
  });

  testWidgets('a blocked author\'s message stays shown in a one-to-one DM '
      'and folds in a group DM', (tester) async {
    api.blocked.add(ann);
    Channel dm(String id, List<String> pubkeys) => Channel(
      id: id,
      name: 'DM',
      channelType: 'dm',
      visibility: 'private',
      description: '',
      createdBy: bob,
      createdAt: DateTime(2025),
      memberCount: pubkeys.length,
      participantPubkeys: pubkeys,
      isMember: true,
    );
    tester.view.physicalSize = const Size(800, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          ...peopleOverrides(
            session: session,
            api: api,
            names: {ann: 'Ann', bob: 'Bob'},
          ),
          channelsProvider.overrideWith(
            () => _FixedChannels([
              dm('one', [bob, ann]),
              dm('group', [bob, ann, cat]),
            ]),
          ),
        ],
        child: MaterialApp(
          theme: AppTheme.light(),
          home: Scaffold(
            body: Column(
              children: [
                BlockedMessageGate(
                  authorPubkey: ann,
                  channelId: 'one',
                  child: const Text('before the block'),
                ),
                BlockedMessageGate(
                  authorPubkey: ann,
                  channelId: 'group',
                  child: const Text('in the group'),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('before the block'), findsOneWidget);
    expect(find.text('in the group'), findsNothing);
    expect(find.text('Message from a blocked user'), findsOneWidget);
  });

  group('settings pages', () {
    testWidgets('Blocked users lists names and unblocks', (tester) async {
      api.blocked.addAll([bob, ann]);
      await pumpPage(tester, const BlockedUsersPage());
      expect(find.text('Ann'), findsOneWidget);
      expect(find.text('Bob'), findsOneWidget);

      await tester.tap(find.byKey(Key('unblock-$ann')));
      await tester.pumpAndSettle();
      expect(api.blocked, {bob});
      expect(find.text('Ann'), findsNothing);
      expect(find.text('Unblocked Ann'), findsOneWidget);
    });

    testWidgets('Blocked users refreshes on pull', (tester) async {
      api.blocked.add(ann);
      await pumpPage(tester, const BlockedUsersPage());
      expect(find.text('Bob'), findsNothing);

      api.blocked.add(bob);
      await tester
          .widget<BeeRefreshIndicator>(find.byType(BeeRefreshIndicator))
          .onRefresh();
      await tester.pumpAndSettle();
      expect(find.text('Ann'), findsOneWidget);
      expect(find.text('Bob'), findsOneWidget);
    });

    testWidgets('Kuzz ID validates and reports a taken ID', (tester) async {
      await pumpPage(tester, const KuzzIdPage(origin: peopleOrigin));
      final field = find.byKey(const Key('kuzz-id-field'));
      expect(find.text('me'), findsOneWidget);

      await tester.enterText(field, 'A!');
      await tester.tap(find.byKey(const Key('kuzz-id-save')));
      await tester.pump();
      expect(find.text('Use 3 to 32 characters.'), findsOneWidget);

      api.usernameError = const AccountApiException(
        'username already taken',
        statusCode: 409,
        code: 'username_taken',
      );
      await tester.enterText(field, 'bob_1');
      await tester.tap(find.byKey(const Key('kuzz-id-save')));
      await tester.pumpAndSettle();
      expect(find.text('That ID is taken'), findsOneWidget);
      expect(api.calls.last, 'username bob_1');
    });

    testWidgets('Friends & privacy shows my ID and saves discoverability', (
      tester,
    ) async {
      await pumpPage(
        tester,
        const Scaffold(
          body: SingleChildScrollView(
            child: Column(children: [PeopleSettings(), SupportSettings()]),
          ),
        ),
      );
      expect(find.text('@me'), findsOneWidget);
      expect(find.text('Blocked users'), findsOneWidget);
      expect(find.text('Contact us'), findsOneWidget);

      await tester.tap(find.byKey(const Key('settings-discoverable-switch')));
      await tester.pumpAndSettle();
      expect(api.calls, contains('discoverable false'));
      expect(
        tester
            .widget<Switch>(
              find.byKey(const Key('settings-discoverable-switch')),
            )
            .value,
        isFalse,
      );
    });

    testWidgets('Friends & privacy is hidden without an account', (
      tester,
    ) async {
      await pumpPage(
        tester,
        const Scaffold(body: PeopleSettings()),
        withAccount: false,
      );
      expect(find.text('Kuzz ID'), findsNothing);
    });

    test('Kuzz ID rules', () {
      expect(kuzzIdError('ab'), isNotNull);
      expect(kuzzIdError('a' * 33), isNotNull);
      expect(kuzzIdError('Ada'), isNotNull);
      expect(kuzzIdError('ada_1'), isNull);
    });
  });
}

class _FixedChannels extends ChannelsNotifier {
  _FixedChannels(this.channels);

  final List<Channel> channels;

  @override
  Future<List<Channel>> build() async => channels;
}
