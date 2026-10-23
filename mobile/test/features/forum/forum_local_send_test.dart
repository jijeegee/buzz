import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_messages_provider.dart';
import 'package:kuzz/features/channels/failed_local_messages_provider.dart';
import 'package:kuzz/features/channels/pending_local_messages_provider.dart';
import 'package:kuzz/features/forum/forum_local_sends.dart';
import 'package:kuzz/features/forum/forum_models.dart';
import 'package:kuzz/features/forum/forum_posts_view.dart';
import 'package:kuzz/features/forum/forum_provider.dart';
import 'package:kuzz/features/forum/forum_thread_page.dart';
import 'package:kuzz/features/profile/profile_provider.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:shared_preferences/shared_preferences.dart';

const _channelId = '22222222-2222-4222-8222-222222222222';
const _otherAuthor =
    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';

void main() {
  late String nsec;
  late String self;
  late SharedPreferences prefs;

  setUp(() async {
    final keys = nostr.Keys.generate();
    nsec = keys.nsec;
    self = keys.public;
    SharedPreferences.setMockInitialValues({});
    prefs = await SharedPreferences.getInstance();
  });
  tearDown(SharedPreferences.resetStatic);

  /// Opens the forum channel as the channel page does: its message provider
  /// (which restores stored sends) is live while the forum is shown.
  Future<(ProviderContainer, _ForumSession)> openForum({
    List<NostrEvent> relayEvents = const [],
    bool postLookupFails = false,
  }) async {
    final session = _ForumSession()
      ..relayEvents.addAll(relayEvents)
      ..postLookupFails = postLookupFails;
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        relayConfigProvider.overrideWith(() => _FixedConfig(nsec)),
        savedPrefsProvider.overrideWithValue(prefs),
        userCacheProvider.overrideWith(_FakeUserCacheNotifier.new),
        profileProvider.overrideWith(_FakeProfileNotifier.new),
        knownAgentPubkeysProvider.overrideWithValue(const {}),
        channelBotPubkeysProvider(
          _channelId,
        ).overrideWith((ref) async => const <String>{}),
        relayClientProvider.overrideWithValue(
          RelayClient(baseUrl: 'http://localhost:3000'),
        ),
      ],
    );
    addTearDown(container.dispose);
    container.listen(channelMessagesProvider(_channelId), (_, _) {});
    await session.subscribed;
    await _pumpEventQueue();
    return (container, session);
  }

  /// Closes [container] and reopens the preferences as a new app run does.
  Future<void> restart(ProviderContainer container) async {
    container.dispose();
    final values = {
      for (final key in prefs.getKeys()) key: prefs.getString(key) as Object,
    };
    SharedPreferences.resetStatic();
    SharedPreferences.setMockInitialValues(values);
    prefs = await SharedPreferences.getInstance();
  }

  Widget postsView(ProviderContainer container) => UncontrolledProviderScope(
    container: container,
    child: MaterialApp(
      theme: AppTheme.light(),
      home: Scaffold(
        body: ForumPostsView(channel: _forumChannel, currentPubkey: self),
      ),
    ),
  );

  Widget threadPage(ProviderContainer container, String postId) =>
      UncontrolledProviderScope(
        container: container,
        child: MaterialApp(
          theme: AppTheme.light(),
          home: ForumThreadPage(
            channelId: _channelId,
            postEventId: postId,
            currentPubkey: self,
            isMember: true,
            isArchived: false,
          ),
        ),
      );

  Future<void> tapFailedMark(WidgetTester tester, String eventId) async {
    await tester.tapAt(
      tester.getCenter(find.byKey(ValueKey('local-send-failed-$eventId'))),
    );
    await tester.pumpAndSettle();
  }

  /// Unmounts the forum and closes the app's providers, whose channel list
  /// keeps a periodic timer, before the test ends.
  Future<void> close(WidgetTester tester, ProviderContainer container) async {
    await tester.pumpWidget(const SizedBox.shrink());
    container.dispose();
    // Lets the unfaked bot-role subscription's retry timer run out.
    await tester.pump(const Duration(seconds: 1));
  }

  testWidgets('a new post shows at once with a clock, then once without it', (
    tester,
  ) async {
    final (container, session) =
        await tester.runAsync(
          () => openForum(relayEvents: [_post('older', createdAt: 1000)]),
        ) ??
        (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pump();
    expect(find.text('older post'), findsOne);

    final send = ForumEventDelivery.capture(
      container,
    ).createPost(channelId: _channelId, content: 'fresh post');
    final event = await session!.nextPublish;
    await tester.pump();

    expect(event.kind, EventKind.forumPost);
    expect(find.text('fresh post'), findsOne);
    expect(find.byKey(ValueKey('local-send-pending-${event.id}')), findsOne);
    expect(
      tester.getTopLeft(find.text('fresh post')).dy,
      lessThan(tester.getTopLeft(find.text('older post')).dy),
    );

    // The relay accepts it; the refetch that will include it is still out.
    final refetch = session.park = Completer<void>();
    session.relayEvents.add(event);
    session.accept();
    await send;
    await tester.pump();
    expect(container.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
    expect(find.text('fresh post'), findsOne);
    expect(find.byIcon(Icons.schedule), findsNothing);

    // Its echo and then the refetch both carry it: still one row.
    session.emit(event);
    refetch.complete();
    await tester.pump();
    await tester.pump();
    expect(find.text('fresh post'), findsOne);
    expect(find.text('older post'), findsOne);
    expect(container.read(forumAcceptedSendsProvider(_channelId)), isEmpty);
    await close(tester, container);
  });

  testWidgets('a failed post stays as Not sent; Retry republishes it', (
    tester,
  ) async {
    final (container, session) =
        await tester.runAsync(openForum) ?? (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pump();

    final send = ForumEventDelivery.capture(
      container,
    ).createPost(channelId: _channelId, content: 'offline post');
    final event = await session!.nextPublish;
    session.reject();
    // The draft is not restored: the post itself keeps the text.
    await expectLater(send, completes);
    await tester.pump();
    expect(find.text('offline post'), findsOne);
    expect(find.text('Not sent'), findsOne);

    await tapFailedMark(tester, event.id);
    await tester.tap(find.text('Retry'));
    await tester.pumpAndSettle();
    final republished = await session.nextPublish;
    expect(republished.id, event.id);
    expect(republished.kind, EventKind.forumPost);
    expect(republished.tags, event.tags);
    await tester.pump();
    expect(find.text('Not sent'), findsNothing);
    expect(find.byIcon(Icons.schedule), findsOne);

    session.relayEvents.add(republished);
    session.accept();
    await tester.pumpAndSettle();
    expect(find.text('offline post'), findsOne);
    expect(find.byIcon(Icons.schedule), findsNothing);
    expect(find.text('Not sent'), findsNothing);
    expect(container.read(forumAcceptedSendsProvider(_channelId)), isEmpty);
    await close(tester, container);
  });

  testWidgets('Delete removes a failed post for good', (tester) async {
    final (container, session) =
        await tester.runAsync(openForum) ?? (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pump();

    unawaited(
      ForumEventDelivery.capture(
        container,
      ).createPost(channelId: _channelId, content: 'unwanted'),
    );
    final event = await session!.nextPublish;
    session.reject();
    await tester.pumpAndSettle();
    expect(find.text('Not sent'), findsOne);

    // A deleted send must not be held as accepted while a refetch runs.
    final refetch = session.park = Completer<void>();
    await tapFailedMark(tester, event.id);
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(find.text('unwanted'), findsNothing);
    expect(container.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
    expect(container.read(forumAcceptedSendsProvider(_channelId)), isEmpty);
    expect(_storedIds(prefs), isEmpty);
    refetch.complete();
    await close(tester, container);
  });

  testWidgets('a post cut off by the app closing comes back as Not sent', (
    tester,
  ) async {
    final (first, firstSession) =
        await tester.runAsync(openForum) ?? (null, null);
    unawaited(
      ForumEventDelivery.capture(
        first!,
      ).createPost(channelId: _channelId, content: 'cut off'),
    );
    final event = await firstSession!.nextPublish;
    expect(_storedIds(prefs), [event.id]);

    final (container, _) =
        await tester.runAsync(() async {
          await restart(first);
          return openForum();
        }) ??
        (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pumpAndSettle();

    expect(find.text('cut off'), findsOne);
    expect(find.text('Not sent'), findsOne);
    expect(
      container.read(failedLocalMessagesProvider(_channelId)),
      contains(event.id),
    );
    await close(tester, container);
  });

  testWidgets('a post that landed before the app closed shows as sent', (
    tester,
  ) async {
    final (first, firstSession) =
        await tester.runAsync(openForum) ?? (null, null);
    unawaited(
      ForumEventDelivery.capture(
        first!,
      ).createPost(channelId: _channelId, content: 'landed'),
    );
    final event = await firstSession!.nextPublish;

    final (container, _) =
        await tester.runAsync(() async {
          await restart(first);
          return openForum(relayEvents: [event]);
        }) ??
        (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pumpAndSettle();

    expect(find.text('landed'), findsOne);
    expect(find.text('Not sent'), findsNothing);
    expect(find.byIcon(Icons.schedule), findsNothing);
    expect(_storedIds(prefs), isEmpty);
    await close(tester, container);
  });

  testWidgets('a reply shows at once in its thread, then as sent', (
    tester,
  ) async {
    final root = _post('root', createdAt: 1000);
    final (container, session) =
        await tester.runAsync(() => openForum(relayEvents: [root])) ??
        (null, null);
    await tester.pumpWidget(threadPage(container!, root.id));
    await tester.pumpAndSettle();
    expect(find.text('No replies yet. Be the first to respond.'), findsOne);

    final send = ForumEventDelivery.capture(container).createReply(
      channelId: _channelId,
      parentEventId: root.id,
      content: 'quick reply',
    );
    final reply = await session!.nextPublish;
    await tester.pump();
    expect(reply.kind, EventKind.forumComment);
    expect(reply.threadReference.parentId, root.id);
    expect(find.text('quick reply'), findsOne);
    expect(find.byKey(ValueKey('local-send-pending-${reply.id}')), findsOne);
    expect(find.text('1 reply'), findsOne);

    final refetch = session.park = Completer<void>();
    session.relayEvents.add(reply);
    session.accept();
    await send;
    await tester.pump();
    expect(find.text('quick reply'), findsOne);
    expect(find.byIcon(Icons.schedule), findsNothing);

    refetch.complete();
    await tester.pumpAndSettle();
    expect(find.text('quick reply'), findsOne);
    expect(find.text('1 reply'), findsOne);
    expect(container.read(forumAcceptedSendsProvider(_channelId)), isEmpty);
    await close(tester, container);
  });

  testWidgets('a failed reply is Not sent and Retry sends it as a reply', (
    tester,
  ) async {
    final root = _post('root', createdAt: 1000);
    final (container, session) =
        await tester.runAsync(() => openForum(relayEvents: [root])) ??
        (null, null);
    await tester.pumpWidget(threadPage(container!, root.id));
    await tester.pumpAndSettle();

    unawaited(
      ForumEventDelivery.capture(container).createReply(
        channelId: _channelId,
        parentEventId: root.id,
        content: 'lost reply',
      ),
    );
    final reply = await session!.nextPublish;
    session.reject();
    await tester.pumpAndSettle();
    expect(find.text('lost reply'), findsOne);
    expect(find.text('Not sent'), findsOne);

    await tapFailedMark(tester, reply.id);
    await tester.tap(find.text('Retry'));
    await tester.pumpAndSettle();
    final republished = await session.nextPublish;
    expect(republished.id, reply.id);
    expect(republished.kind, EventKind.forumComment);
    expect(republished.tags, reply.tags);
    await close(tester, container);
  });

  /// A reply to [root] that failed to send, then a restart with the relay
  /// holding [relayEvents].
  Future<(ProviderContainer, NostrEvent)> restartWithFailedReply(
    WidgetTester tester, {
    required List<NostrEvent> relayEvents,
    bool postLookupFails = false,
  }) async {
    final root = _post('root', createdAt: 1000);
    final (first, firstSession) =
        await tester.runAsync(() => openForum(relayEvents: [root])) ??
        (null, null);
    unawaited(
      ForumEventDelivery.capture(first!).createReply(
        channelId: _channelId,
        parentEventId: root.id,
        content: 'lost reply',
      ),
    );
    final reply = await firstSession!.nextPublish;
    firstSession.reject();
    await tester.runAsync(_pumpEventQueue);
    expect(_storedIds(prefs), [reply.id]);

    final (container, _) =
        await tester.runAsync(() async {
          await restart(first);
          final opened = await openForum(
            relayEvents: relayEvents,
            postLookupFails: postLookupFails,
          );
          for (var i = 0; i < 5; i++) {
            await _pumpEventQueue();
          }
          return opened;
        }) ??
        (null, null);
    return (container!, reply);
  }

  testWidgets('a failed reply to a deleted post is dropped on restart', (
    tester,
  ) async {
    final (container, reply) = await restartWithFailedReply(
      tester,
      relayEvents: const [],
    );
    expect(
      container.read(pendingLocalMessagesProvider(_channelId)),
      isNot(contains(reply.id)),
    );
    expect(_storedIds(prefs), isEmpty);
    container.dispose();
  });

  testWidgets('a failed reply to a listed post stays Not sent on restart', (
    tester,
  ) async {
    final (container, reply) = await restartWithFailedReply(
      tester,
      relayEvents: [_post('root', createdAt: 1000)],
    );
    expect(
      container.read(failedLocalMessagesProvider(_channelId)),
      contains(reply.id),
    );
    expect(_storedIds(prefs), [reply.id]);
    container.dispose();
  });

  testWidgets('a failed reply stays when its post cannot be looked up', (
    tester,
  ) async {
    final (container, reply) = await restartWithFailedReply(
      tester,
      relayEvents: const [],
      postLookupFails: true,
    );
    expect(
      container.read(failedLocalMessagesProvider(_channelId)),
      contains(reply.id),
    );
    expect(_storedIds(prefs), [reply.id]);
    container.dispose();
  });

  testWidgets('own rows and their Retry stay when a list refresh fails', (
    tester,
  ) async {
    final root = _post('root', createdAt: 1000);
    final (container, session) =
        await tester.runAsync(() => openForum(relayEvents: [root])) ??
        (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pump();

    unawaited(
      ForumEventDelivery.capture(
        container,
      ).createPost(channelId: _channelId, content: 'offline post'),
    );
    final event = await session!.nextPublish;
    session.reject();
    await tester.pumpAndSettle();
    expect(find.text('Not sent'), findsOne);

    session.listError = Exception('offline');
    container.invalidate(forumPostsProvider(_channelId));
    await tester.pump();
    await tester.pump();
    expect(container.read(forumPostsProvider(_channelId)).hasError, isTrue);
    expect(find.byKey(const ValueKey('load-error-retry')), findsNothing);
    expect(find.text('Failed to load posts'), findsOne);
    expect(find.text('root post'), findsOne);
    expect(find.text('offline post'), findsOne);
    await tapFailedMark(tester, event.id);
    expect(find.text('Retry'), findsWidgets);
    expect(find.text('Delete'), findsOne);
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(find.text('offline post'), findsNothing);

    // The thread page keeps its loaded thread and own replies the same way.
    session.listError = null;
    await tester.pumpWidget(threadPage(container, root.id));
    await tester.pumpAndSettle();
    unawaited(
      ForumEventDelivery.capture(container).createReply(
        channelId: _channelId,
        parentEventId: root.id,
        content: 'offline reply',
      ),
    );
    await session.nextPublish;
    session.reject();
    await tester.pumpAndSettle();
    session.listError = Exception('offline');
    container.invalidate(
      forumThreadProvider((channelId: _channelId, eventId: root.id)),
    );
    await tester.pump();
    await tester.pump();
    expect(find.text('Failed to load thread'), findsOne);
    expect(find.text('root post'), findsOne);
    expect(find.text('offline reply'), findsOne);
    expect(find.text('Not sent'), findsOne);
    session.listError = null;
    await close(tester, container);
  });

  testWidgets('a list with a "Not sent" post confirms it as sent', (
    tester,
  ) async {
    final (container, session) =
        await tester.runAsync(openForum) ?? (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pump();

    unawaited(
      ForumEventDelivery.capture(
        container,
      ).createPost(channelId: _channelId, content: 'stored anyway'),
    );
    final event = await session!.nextPublish;
    // The relay stores it, but its OK is lost.
    session.relayEvents.add(event);
    session.reject();
    await tester.pumpAndSettle();
    expect(find.text('Not sent'), findsOne);
    expect(_storedIds(prefs), [event.id]);

    // The next poll lists it: no retry, so no second copy, is needed.
    container.invalidate(forumPostsProvider(_channelId));
    await tester.pumpAndSettle();
    expect(find.text('stored anyway'), findsOne);
    expect(find.text('Not sent'), findsNothing);
    expect(container.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
    expect(container.read(failedLocalMessagesProvider(_channelId)), isEmpty);
    expect(_storedIds(prefs), isEmpty);
    await close(tester, container);
  });

  testWidgets('a "Not sent" post whose echo arrives stays until listed', (
    tester,
  ) async {
    final (container, session) =
        await tester.runAsync(openForum) ?? (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pump();

    unawaited(
      ForumEventDelivery.capture(
        container,
      ).createPost(channelId: _channelId, content: 'late echo'),
    );
    final event = await session!.nextPublish;
    session.reject();
    await tester.pumpAndSettle();
    expect(find.text('Not sent'), findsOne);

    // The relay had it after all: its echo confirms the failed send while
    // the refetch that lists it is still out.
    final refetch = session.park = Completer<void>();
    session.relayEvents.add(event);
    session.emit(event);
    await tester.pump();
    expect(container.read(pendingLocalMessagesProvider(_channelId)), isEmpty);
    expect(container.read(forumAcceptedSendsProvider(_channelId)).keys, [
      event.id,
    ]);
    expect(find.text('late echo'), findsOne);
    expect(find.text('Not sent'), findsNothing);

    refetch.complete();
    await tester.pumpAndSettle();
    expect(find.text('late echo'), findsOne);
    expect(container.read(forumAcceptedSendsProvider(_channelId)), isEmpty);
    await close(tester, container);
  });

  testWidgets('an accepted reply refetches no thread that is not loaded', (
    tester,
  ) async {
    final root = _post('root', createdAt: 1000);
    final (container, session) =
        await tester.runAsync(() => openForum(relayEvents: [root])) ??
        (null, null);
    await tester.pumpWidget(postsView(container!));
    await tester.pump();

    final send = ForumEventDelivery.capture(container).createReply(
      channelId: _channelId,
      parentEventId: root.id,
      content: 'unseen reply',
    );
    final reply = await session!.nextPublish;
    session.relayEvents.add(reply);
    session.accept();
    await send;
    await tester.pumpAndSettle();
    final thread = forumThreadProvider((
      channelId: _channelId,
      eventId: root.id,
    ));
    expect(container.exists(thread), isFalse);
    expect(container.read(forumAcceptedSendsProvider(_channelId)), isEmpty);
    await close(tester, container);
  });

  group('merging local sends into fetched lists', () {
    test('posts keep newest first and skip ones already fetched', () {
      final fetched = [
        ForumPost.fromEvent(_post('b', createdAt: 20)),
        ForumPost.fromEvent(_post('a', createdAt: 10)),
      ];
      final merged = withLocalForumPosts(fetched, [
        _post('b', createdAt: 20),
        _post('c', createdAt: 30),
        _post('m', createdAt: 15),
        _reply('r', parent: 'a', createdAt: 40),
      ]);
      expect([for (final post in merged) post.eventId], ['c', 'b', 'm', 'a']);
    });

    test('replies keep oldest first and only answer the open post', () {
      final fetched = [
        ThreadReply.fromEvent(_reply('x', parent: 'p', createdAt: 10)),
      ];
      final merged = withLocalForumReplies(fetched, [
        _reply('x', parent: 'p', createdAt: 10),
        _reply('y', parent: 'p', createdAt: 20),
        _reply('z', parent: 'other', createdAt: 30),
        _post('q', createdAt: 40),
      ], 'p');
      expect([for (final reply in merged) reply.eventId], ['x', 'y']);
    });
  });
}

/// Ids of the local sends kept on the device for any signer.
List<String> _storedIds(SharedPreferences prefs) => [
  for (final key in prefs.getKeys())
    if (key.startsWith('local_sends_v1:'))
      for (final entry in jsonDecode(prefs.getString(key)!) as List<dynamic>)
        (entry as Map<String, dynamic>)['id'] as String,
];

final _forumChannel = Channel(
  id: _channelId,
  name: 'design-forum',
  channelType: 'forum',
  visibility: 'open',
  description: '',
  createdBy: _otherAuthor,
  createdAt: DateTime(2025),
  memberCount: 2,
  isMember: true,
);

NostrEvent _post(String id, {required int createdAt}) => NostrEvent(
  id: id,
  pubkey: _otherAuthor,
  createdAt: createdAt,
  kind: EventKind.forumPost,
  tags: const [
    ['h', _channelId],
  ],
  content: '$id post',
  sig: 'sig',
);

NostrEvent _reply(
  String id, {
  required String parent,
  required int createdAt,
}) => NostrEvent(
  id: id,
  pubkey: _otherAuthor,
  createdAt: createdAt,
  kind: EventKind.forumComment,
  tags: [
    const ['h', _channelId],
    ['e', parent, '', 'reply'],
  ],
  content: '$id reply',
  sig: 'sig',
);

class _FixedConfig extends RelayConfigNotifier {
  final String nsec;

  _FixedConfig(this.nsec);

  @override
  RelayConfig build() =>
      RelayConfig(baseUrl: 'https://relay.example', nsec: nsec);
}

class _FakeUserCacheNotifier extends UserCacheNotifier {
  @override
  Map<String, UserProfile> build() => const {};

  @override
  UserProfile? get(String pubkey) => null;
}

class _FakeProfileNotifier extends ProfileNotifier {
  @override
  Future<UserProfile?> build() async => null;
}

/// A connected relay holding [relayEvents], whose publishes the test answers
/// one at a time. Forum list and thread queries wait on [park] when set.
class _ForumSession extends RelaySessionNotifier {
  final relayEvents = <NostrEvent>[];
  Completer<void>? park;

  /// Thrown by forum list and thread queries while set.
  Object? listError;

  /// Fails by-id lookups of forum posts, as an unreachable relay would.
  var postLookupFails = false;
  final _listeners = <void Function(NostrEvent)>[];
  final _subscribed = Completer<void>();
  final _announced = <Completer<NostrEvent>>[];
  var _publishCount = 0;
  var _readCount = 0;
  Completer<NostrEvent>? _current;
  NostrEvent? _currentEvent;

  Future<void> get subscribed => _subscribed.future;

  Completer<NostrEvent> _slot(int index) {
    while (_announced.length <= index) {
      _announced.add(Completer<NostrEvent>());
    }
    return _announced[index];
  }

  /// The next event handed to [publish] that the test hasn't read yet.
  Future<NostrEvent> get nextPublish => _slot(_readCount++).future;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  List<NostrEvent> _matching(NostrFilter filter) {
    final roots = filter.tags['#e'];
    return [
      for (final event in relayEvents)
        if (filter.kinds.contains(event.kind) &&
            (filter.ids == null || filter.ids!.contains(event.id)) &&
            (roots == null ||
                event.tags.any(
                  (tag) =>
                      tag.length > 1 && tag[0] == 'e' && roots.contains(tag[1]),
                )))
          event,
    ];
  }

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final filter = filters.single;
    if (postLookupFails && filter.kinds.contains(EventKind.forumPost)) {
      throw Exception('relay unreachable');
    }
    if (filter.ids != null) return _matching(filter);
    return [_bounds()];
  }

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final isForumList =
        filter.ids == null &&
        (filter.kinds.contains(EventKind.forumPost) ||
            filter.tags.containsKey('#e'));
    if (isForumList) {
      if (park case final park?) await park.future;
      if (listError case final error?) throw error;
    }
    return _matching(filter);
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    if (!_subscribed.isCompleted) _subscribed.complete();
    _listeners.add(onEvent);
    return () => _listeners.remove(onEvent);
  }

  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) {
    final result = Completer<NostrEvent>();
    _current = result;
    _currentEvent = event;
    _slot(_publishCount++).complete(event);
    return result.future;
  }

  void accept() => _current!.complete(_currentEvent!);

  void reject() => _current!.completeError(Exception('relay rejected event'));

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}

NostrEvent _bounds() => NostrEvent(
  id: 'bounds',
  pubkey: 'relay',
  createdAt: 0,
  kind: EventKind.channelWindowBounds,
  tags: [
    ['d', '${_channelId.toLowerCase()}:head'],
  ],
  content: jsonEncode({'has_more': false, 'next_cursor': null}),
  sig: 'sig',
);

Future<void> _pumpEventQueue() async {
  await Future<void>.delayed(Duration.zero);
  await Future<void>.delayed(Duration.zero);
}
