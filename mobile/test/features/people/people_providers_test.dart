import 'dart:io';

import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/activity/activity_provider.dart';
import 'package:kuzz/features/activity/feed_item.dart';
import 'package:kuzz/features/people/people_errors.dart';
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

    test('a change keeps content, other tags and petnames', () async {
      final container = containerFor();
      session.follow(
        peopleKeys.public,
        const [],
        content: '{"wss://relay.test":{"read":true}}',
        tags: [
          ['p', ann, 'wss://hint.test', 'annie'],
          ['t', 'kuzz'],
          ['p', cat.toUpperCase(), '', 'cathy'],
        ],
      );

      await container.read(friendsControllerProvider).add(bob);
      final added = session.published.last;
      expect(added.content, '{"wss://relay.test":{"read":true}}');
      expect(added.tags, [
        ['p', ann, 'wss://hint.test', 'annie'],
        ['t', 'kuzz'],
        ['p', cat.toUpperCase(), '', 'cathy'],
        ['p', bob],
      ]);

      await container.read(friendsControllerProvider).remove(cat);
      expect(session.published.last.tags, [
        ['p', ann, 'wss://hint.test', 'annie'],
        ['t', 'kuzz'],
        ['p', bob],
      ]);
    });

    test('friendIdsProvider reads my newest list', () async {
      final container = containerFor();
      session.follow(peopleKeys.public, [ann], createdAt: 100);
      session.follow(peopleKeys.public, [ann, bob], createdAt: 300);
      container.listen(friendIdsProvider, (_, _) {});
      await pumpEventQueue();

      expect(container.read(friendIdsProvider).value, {ann, bob});
    });

    test('a change from another device arrives live', () async {
      final container = containerFor();
      session.follow(peopleKeys.public, [ann], createdAt: 100);
      container.listen(friendIdsProvider, (_, _) {});
      await pumpEventQueue();
      expect(container.read(friendIdsProvider).value, {ann});

      session.follow(peopleKeys.public, [ann, bob], createdAt: 200);
      session.deliver(session.contacts.last);
      await pumpEventQueue();
      expect(container.read(friendIdsProvider).value, {ann, bob});
    });

    test('returning to the foreground re-reads the list', () async {
      final container = containerFor();
      session.follow(peopleKeys.public, [ann], createdAt: 100);
      container.listen(friendIdsProvider, (_, _) {});
      await pumpEventQueue();

      // Changed elsewhere while the event was missed.
      session.follow(peopleKeys.public, [bob], createdAt: 200);
      final lifecycle =
          container.read(appLifecycleProvider.notifier) as ManualLifecycle;
      lifecycle.set(AppLifecycleState.paused);
      lifecycle.set(AppLifecycleState.resumed);
      await pumpEventQueue();
      expect(container.read(friendIdsProvider).value, {bob});
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
      expect(api.calls, ['list', 'block $bob', 'list', 'unblock $ann', 'list']);
    });

    test('a block after a failed load shows the server list', () async {
      api.blocked.add(ann);
      api.listFailures = 1;
      final container = containerFor();
      container.listen(blockedUsersProvider, (_, _) {});
      await expectLater(
        container.read(blockedUsersProvider.future),
        throwsA(isA<AccountApiException>()),
      );

      await container.read(blockedUsersProvider.notifier).block(bob);
      expect(container.read(blockedPubkeysProvider), {ann, bob});
    });

    test('a failed load is retried on resume and on reconnect', () async {
      api.blocked.add(ann);
      api.listFailures = 2;
      final container = containerFor();
      container.listen(blockedUsersProvider, (_, _) {});
      await expectLater(
        container.read(blockedUsersProvider.future),
        throwsA(isA<AccountApiException>()),
      );

      final lifecycle =
          container.read(appLifecycleProvider.notifier) as ManualLifecycle;
      lifecycle.set(AppLifecycleState.paused);
      lifecycle.set(AppLifecycleState.resumed);
      await pumpEventQueue();
      // The second read fails too; still nothing hidden.
      expect(container.read(blockedPubkeysProvider), isEmpty);

      session.setStatus(SessionStatus.disconnected);
      session.setStatus(SessionStatus.connected);
      await pumpEventQueue();
      expect(container.read(blockedPubkeysProvider), {ann});
      expect(api.calls, ['list', 'list', 'list']);
    });

    test('a good list is re-read quietly on resume', () async {
      api.blocked.add(ann);
      final container = containerFor();
      final states = <AsyncValue<Set<String>>>[];
      container.listen(blockedUsersProvider, (_, next) => states.add(next));
      expect(await container.read(blockedUsersProvider.future), {ann});

      // Blocked on another device.
      api.blocked.add(bob);
      final lifecycle =
          container.read(appLifecycleProvider.notifier) as ManualLifecycle;
      lifecycle.set(AppLifecycleState.paused);
      lifecycle.set(AppLifecycleState.resumed);
      await pumpEventQueue();
      expect(container.read(blockedPubkeysProvider), {ann, bob});
      expect(states.where((s) => s.isLoading), isEmpty);
    });

    test('a failed refresh keeps the list shown', () async {
      api.blocked.add(ann);
      final container = containerFor();
      container.listen(blockedUsersProvider, (_, _) {});
      expect(await container.read(blockedUsersProvider.future), {ann});

      api.listFailures = 1;
      await container.read(blockedUsersProvider.notifier).refresh();
      expect(container.read(blockedUsersProvider).value, {ann});
      expect(container.read(blockedUsersProvider).hasError, isFalse);
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

  group('error text', () {
    test('maps statuses to fixed sentences', () {
      String text(Object error) => friendlyError(error, fallback: 'Fallback.');
      expect(
        text(
          const AccountApiException(
            'block limit reached',
            statusCode: 409,
            code: 'block_limit',
          ),
        ),
        'You’ve blocked the most people you can.',
      );
      expect(
        text(
          const AccountApiException(
            'taken',
            statusCode: 409,
            code: 'username_taken',
          ),
        ),
        'That ID is taken',
      );
      expect(
        text(const AccountApiException('slow down', statusCode: 429)),
        'Too many tries. Wait a minute and try again.',
      );
      expect(
        text(const AccountApiException('no', statusCode: 403)),
        'You don’t have permission to do that.',
      );
      expect(
        text(const AccountApiException('offline')),
        couldNotConnectMessage,
      );
      expect(
        text(const AccountApiException('boom', statusCode: 502)),
        couldNotConnectMessage,
      );
      expect(
        text(const SocketException('Failed host lookup')),
        couldNotConnectMessage,
      );
      expect(text(Exception('Publish timed out')), couldNotConnectMessage);
      expect(text(Exception('invalid: bad event')), 'Fallback.');
    });
  });

  test('activity leaves out items by blocked people', () {
    FeedItem item(String id, String author) => FeedItem(
      id: id,
      kind: 1,
      pubkey: author,
      content: 'hi',
      createdAt: 1,
      channelId: 'c',
      channelName: 'general',
      tags: const [],
      category: 'mention',
    );
    final items = [item('1', ann), item('2', bob.toUpperCase())];
    expect(withoutBlockedAuthors(items, {bob}).map((i) => i.id).toList(), [
      '1',
    ]);
    expect(withoutBlockedAuthors(items, const {}).length, 2);
  });
}
