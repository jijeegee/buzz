import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/people/blocked_users_provider.dart';
import 'package:kuzz/features/people/friends_provider.dart';
import 'package:kuzz/features/people/report.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/relay/relay.dart';

import 'people_fixture.dart';

void main() {
  late PeopleSession session;
  late FakePeopleApi api;

  ProviderContainer containerFor({bool withAccount = true}) {
    final container = ProviderContainer(
      overrides: peopleOverrides(
        session: session,
        api: withAccount ? api : null,
      ),
    );
    addTearDown(container.dispose);
    return container;
  }

  setUp(() {
    session = PeopleSession();
    api = FakePeopleApi();
  });

  group('report tags', () {
    test('a person report carries the type on its only p tag', () {
      expect(reportTags(pubkey: ann.toUpperCase(), reason: ReportReason.spam), [
        ['p', ann, 'spam'],
      ]);
    });

    test('a message report names the author and types the e tag', () {
      expect(
        reportTags(
          pubkey: ann,
          reason: ReportReason.impersonation,
          eventId: 'e1',
        ),
        [
          ['p', ann],
          ['e', 'e1', 'impersonation'],
        ],
      );
    });

    test('every reason is a type the relay accepts', () {
      expect(ReportReason.values.map((r) => r.wireValue).toSet(), {
        'illegal',
        'nudity',
        'malware',
        'spam',
        'impersonation',
        'profanity',
        'other',
      });
    });

    test('the service signs a kind 1984 with empty content', () async {
      final container = containerFor();
      await container
          .read(reportServiceProvider)
          .submit(pubkey: bob, reason: ReportReason.other, eventId: 'e9');

      final event = session.published.single;
      expect(event.kind, EventKind.report);
      expect(event.content, isEmpty);
      expect(event.pubkey, peopleKeys.public);
      expect(event.tags, [
        ['p', bob],
        ['e', 'e9', 'other'],
      ]);
    });
  });

  group('friends', () {
    test('add re-reads the latest list and keeps everyone on it', () async {
      final container = containerFor();
      session.follow(peopleKeys.public, [ann], createdAt: 100);
      // Another device added cat after this one last looked.
      session.follow(peopleKeys.public, [ann, cat], createdAt: 200);

      await container.read(friendsControllerProvider).add(bob.toUpperCase());

      final written = session.published.single;
      expect(written.kind, EventKind.contactList);
      expect(written.tags.map((t) => t[1]), [ann, cat, bob]);
    });

    test('remove drops one person; a no-op writes nothing', () async {
      final container = containerFor();
      session.follow(peopleKeys.public, [ann, bob]);

      await container.read(friendsControllerProvider).remove(cat);
      expect(session.published, isEmpty);

      await container.read(friendsControllerProvider).remove(ann);
      expect(session.published.single.tags.map((t) => t[1]), [bob]);
    });

    test('friendIdsProvider reads my newest list', () async {
      final container = containerFor();
      session.follow(peopleKeys.public, [ann], createdAt: 100);
      session.follow(peopleKeys.public, [ann, bob], createdAt: 300);
      container.listen(friendIdsProvider, (_, _) {});
      await pumpEventQueue();

      expect(container.read(friendIdsProvider).value, {ann, bob});
    });
  });

  group('blocked users', () {
    test('loads from the account and updates after block/unblock', () async {
      api.blocked.add(ann);
      final container = containerFor();
      expect(await container.read(blockedUsersProvider.future), {ann});

      await container.read(blockedUsersProvider.notifier).block(bob);
      expect(container.read(blockedPubkeysProvider), {ann, bob});

      await container.read(blockedUsersProvider.notifier).unblock(ann);
      expect(container.read(blockedPubkeysProvider), {bob});
      expect(api.calls, ['list', 'block $bob', 'unblock $ann']);
    });

    test('a refused block leaves the list unchanged', () async {
      api.blockError = const AccountApiException('nope', statusCode: 400);
      final container = containerFor();
      await container.read(blockedUsersProvider.future);

      await expectLater(
        container.read(blockedUsersProvider.notifier).block(bob),
        throwsA(isA<AccountApiException>()),
      );
      expect(container.read(blockedPubkeysProvider), isEmpty);
    });

    test('without an account nothing is blocked and blocking fails', () async {
      final container = containerFor(withAccount: false);
      expect(await container.read(blockedUsersProvider.future), isEmpty);
      await expectLater(
        container.read(blockedUsersProvider.notifier).block(bob),
        throwsA(isA<AccountApiException>()),
      );
    });
  });
}
