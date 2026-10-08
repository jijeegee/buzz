import 'dart:async';

import 'package:buzz/features/channels/channel_identity_names_provider.dart';
import 'package:buzz/features/channels/composer_quote_chip.dart';
import 'package:buzz/features/channels/composer_quote_provider.dart';
import 'package:buzz/features/channels/message_quote.dart';
import 'package:buzz/features/channels/message_quote_header.dart';
import 'package:buzz/features/channels/quoted_message_provider.dart';
import 'package:buzz/features/channels/timeline_message.dart';
import 'package:buzz/shared/deeplink/deep_link.dart';
import 'package:buzz/shared/deeplink/pending_deep_link_provider.dart';
import 'package:buzz/shared/identity_names/identity_names.dart';
import 'package:buzz/shared/profile/user_cache_provider.dart';
import 'package:buzz/shared/profile/user_profile.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/misc.dart';

import '../../helpers/widget_helpers.dart';

const _channelId = '11111111-1111-4111-8111-111111111111';
final _alice = 'a' * 64;
final _bob = 'b' * 64;
final _quotedId = 'c' * 64;
final _rootId = 'd' * 64;
final _quotingId = 'e' * 64;

TimelineMessage _message({
  required String id,
  required String pubkey,
  String content = 'hello world',
  bool isSystem = false,
  String? rootId,
  List<List<String>> tags = const [],
}) => TimelineMessage(
  id: id,
  pubkey: pubkey,
  createdAt: 1000,
  content: content,
  isSystem: isSystem,
  rootId: rootId,
  parentId: rootId,
  tags: tags,
);

void main() {
  group('buildQuoteTag', () {
    test('builds a normalized NIP-18 tag', () {
      expect(
        buildQuoteTag(
          eventId: ' ${_quotedId.toUpperCase()} ',
          authorPubkey: _alice.toUpperCase(),
        ),
        ['q', _quotedId, '', _alice],
      );
    });

    test('rejects malformed ids and authors', () {
      expect(
        () => buildQuoteTag(eventId: 'short', authorPubkey: _alice),
        throwsArgumentError,
      );
      expect(
        () => buildQuoteTag(eventId: _quotedId, authorPubkey: 'short'),
        throwsArgumentError,
      );
    });
  });

  group('quoteReferenceOf', () {
    test('reads the first well-formed q tag', () {
      expect(
        quoteReferenceOf([
          ['h', _channelId],
          ['q', 'not-hex', '', _alice],
          ['q', _quotedId.toUpperCase(), '', _alice],
        ]),
        QuoteReference(eventId: _quotedId, authorPubkey: _alice),
      );
    });

    test('tolerates a missing author and ignores e tags', () {
      expect(
        quoteReferenceOf([
          ['q', _quotedId],
        ]),
        QuoteReference(eventId: _quotedId),
      );
      expect(
        quoteReferenceOf([
          ['e', _quotedId, '', 'reply'],
        ]),
        isNull,
      );
    });
  });

  group('quoteTargetFor', () {
    test('quotes ordinary messages with hex ids', () {
      final target = quoteTargetFor(
        _message(id: _quotedId, pubkey: _alice, content: '**Hi**\nthere'),
        author: 'Alice',
      );
      expect(target?.eventId, _quotedId);
      expect(target?.authorPubkey, _alice);
      expect(target?.author, 'Alice');
      expect(target?.excerpt, 'Hi there');
    });

    test('does not quote system or non-hex messages', () {
      expect(
        quoteTargetFor(
          _message(id: _quotedId, pubkey: _alice, isSystem: true),
          author: 'Alice',
        ),
        isNull,
      );
      expect(
        quoteTargetFor(
          _message(id: 'msg-1', pubkey: _alice),
          author: 'A',
        ),
        isNull,
      );
    });
  });

  group('quoteExcerpt', () {
    test('flattens markdown and links into one line', () {
      expect(
        quoteExcerpt('# Title\n[docs](https://x.y) and https://a.b/c `code`'),
        'Title docs and code',
      );
      expect(quoteExcerpt('   '), 'No message text');
    });

    test('clips long content with an ellipsis', () {
      final excerpt = quoteExcerpt('word ' * 100);
      expect(excerpt.length, lessThanOrEqualTo(160));
      expect(excerpt, endsWith('…'));
    });
  });

  group('composerQuoteProvider', () {
    test('clearIf only clears the quote that was sent', () {
      final container = ProviderContainer();
      addTearDown(container.dispose);
      const scope = ComposerQuoteScope(channelId: _channelId);
      final subscription = container.listen(
        composerQuoteProvider(scope),
        (_, _) {},
      );
      addTearDown(subscription.close);
      final notifier = container.read(composerQuoteProvider(scope).notifier);
      final first = QuoteTarget(
        eventId: _quotedId,
        authorPubkey: _alice,
        author: 'Alice',
        excerpt: 'one',
      );
      final second = QuoteTarget(
        eventId: _rootId,
        authorPubkey: _bob,
        author: 'Bob',
        excerpt: 'two',
      );

      notifier.quote(first);
      notifier.quote(second);
      notifier.clearIf(first);
      expect(container.read(composerQuoteProvider(scope)), second);
      notifier.clearIf(second);
      expect(container.read(composerQuoteProvider(scope)), isNull);
    });

    test('keeps main-timeline and thread quotes separate', () {
      final container = ProviderContainer();
      addTearDown(container.dispose);
      const main = ComposerQuoteScope(channelId: _channelId);
      final thread = ComposerQuoteScope(
        channelId: _channelId,
        threadHeadId: _rootId,
      );
      final subscriptions = [
        container.listen(composerQuoteProvider(main), (_, _) {}),
        container.listen(composerQuoteProvider(thread), (_, _) {}),
      ];
      addTearDown(() {
        for (final subscription in subscriptions) {
          subscription.close();
        }
      });

      container
          .read(composerQuoteProvider(thread).notifier)
          .quote(
            QuoteTarget(
              eventId: _quotedId,
              authorPubkey: _alice,
              author: 'Alice',
              excerpt: 'x',
            ),
          );
      expect(container.read(composerQuoteProvider(main)), isNull);
      expect(container.read(composerQuoteProvider(thread)), isNotNull);
    });
  });

  group('ComposerQuoteChip', () {
    testWidgets('shows the quoted author and excerpt and can be removed', (
      tester,
    ) async {
      const scope = ComposerQuoteScope(channelId: _channelId);
      late WidgetRef widgetRef;
      await tester.pumpWidget(
        WidgetHelpers.testable(
          child: Consumer(
            builder: (context, ref, _) {
              widgetRef = ref;
              return const ComposerQuoteChip(scope: scope);
            },
          ),
        ),
      );
      expect(find.byKey(const ValueKey('composer-quote-chip')), findsNothing);

      widgetRef
          .read(composerQuoteProvider(scope).notifier)
          .quote(
            QuoteTarget(
              eventId: _quotedId,
              authorPubkey: _alice,
              author: 'Alice',
              excerpt: 'The original message',
            ),
          );
      await tester.pump();

      expect(find.text('Quoting Alice'), findsOneWidget);
      expect(find.text('The original message'), findsOneWidget);

      await tester.tap(find.byTooltip('Cancel quote'));
      await tester.pump();

      expect(find.text('Quoting Alice'), findsNothing);
      expect(widgetRef.read(composerQuoteProvider(scope)), isNull);
    });
  });

  group('MessageQuoteHeader', () {
    late StreamController<Uri> uriLinks;

    setUp(() {
      uriLinks = StreamController<Uri>.broadcast();
      PendingDeepLinkNotifier.debugUriStreamOverride = uriLinks.stream;
    });

    tearDown(() async {
      PendingDeepLinkNotifier.debugUriStreamOverride = null;
      await uriLinks.close();
    });

    List<Override> overrides({QuotedMessage? fetched}) => [
      userCacheProvider.overrideWith(
        () => _FixedUserCacheNotifier({
          _alice: UserProfile(pubkey: _alice, displayName: 'Alice'),
        }),
      ),
      channelIdentityNamesProvider(_channelId).overrideWithValue(
        IdentityNameSources(
          profiles: {_alice: UserProfile(pubkey: _alice, displayName: 'Alice')},
        ).scope([_alice, _bob]),
      ),
      quotedMessageProvider.overrideWith((ref, _) async => fetched),
    ];

    Future<ProviderContainer> pumpHeader(
      WidgetTester tester, {
      List<TimelineMessage>? loadedMessages,
      QuotedMessage? fetched,
      List<List<String>>? tags,
      QuoteJumpScope Function(Widget child)? wrap,
    }) async {
      final header = MessageQuoteHeader(
        channelId: _channelId,
        tags:
            tags ??
            [
              ['h', _channelId],
              ['q', _quotedId, '', _alice],
            ],
        loadedMessages: loadedMessages,
      );
      await tester.pumpWidget(
        WidgetHelpers.testable(
          overrides: overrides(fetched: fetched),
          child: wrap?.call(header) ?? header,
        ),
      );
      await tester.pumpAndSettle();
      return ProviderScope.containerOf(
        tester.element(find.byType(MessageQuoteHeader)),
      );
    }

    testWidgets('renders nothing without a q tag', (tester) async {
      await pumpHeader(
        tester,
        tags: [
          ['h', _channelId],
        ],
      );
      expect(find.byType(InkWell), findsNothing);
      expect(find.textContaining('quoted'), findsNothing);
      expect(find.text('Original message unavailable'), findsNothing);
    });

    testWidgets('resolves a loaded message and opens it on tap', (
      tester,
    ) async {
      final container = await pumpHeader(
        tester,
        loadedMessages: [
          _message(
            id: _quotedId,
            pubkey: _alice,
            content: 'The **original** text',
            rootId: _rootId,
          ),
        ],
      );

      expect(find.text('Alice'), findsOneWidget);
      expect(find.text('The original text'), findsOneWidget);

      await tester.tap(find.byKey(ValueKey('message-quote-header-$_quotedId')));
      await tester.pump();

      expect(
        container.read(pendingDeepLinkProvider),
        MessageDeepLink(
          channelId: _channelId,
          messageId: _quotedId,
          threadRootId: _rootId,
        ),
      );
    });

    testWidgets('scrolls in place when the enclosing list shows it', (
      tester,
    ) async {
      final jumps = <String>[];
      final container = await pumpHeader(
        tester,
        loadedMessages: [_message(id: _quotedId, pubkey: _alice)],
        wrap: (child) => QuoteJumpScope(
          channelId: _channelId,
          jumpToMessage: (messageId) {
            jumps.add(messageId);
            return true;
          },
          child: child,
        ),
      );

      await tester.tap(find.byKey(ValueKey('message-quote-header-$_quotedId')));
      await tester.pump();

      expect(jumps, [_quotedId]);
      expect(container.read(pendingDeepLinkProvider), isNull);
    });

    testWidgets('opens the deep link when the enclosing list lacks it', (
      tester,
    ) async {
      final jumps = <String>[];
      final container = await pumpHeader(
        tester,
        loadedMessages: [_message(id: _quotedId, pubkey: _alice)],
        wrap: (child) => QuoteJumpScope(
          channelId: _channelId,
          jumpToMessage: (messageId) {
            jumps.add(messageId);
            return false;
          },
          child: child,
        ),
      );

      await tester.tap(find.byKey(ValueKey('message-quote-header-$_quotedId')));
      await tester.pump();

      expect(jumps, [_quotedId]);
      expect(
        container.read(pendingDeepLinkProvider),
        MessageDeepLink(channelId: _channelId, messageId: _quotedId),
      );
    });

    testWidgets('opens the deep link for a quote from another channel', (
      tester,
    ) async {
      const otherChannelId = '22222222-2222-4222-8222-222222222222';
      final jumps = <String>[];
      final container = await pumpHeader(
        tester,
        fetched: (
          channelId: otherChannelId,
          message: _message(id: _quotedId, pubkey: _alice, content: 'Fetched'),
        ),
        wrap: (child) => QuoteJumpScope(
          channelId: _channelId,
          jumpToMessage: (messageId) {
            jumps.add(messageId);
            return true;
          },
          child: child,
        ),
      );

      await tester.tap(find.byKey(ValueKey('message-quote-header-$_quotedId')));
      await tester.pump();

      expect(jumps, isEmpty);
      expect(
        container.read(pendingDeepLinkProvider),
        MessageDeepLink(channelId: otherChannelId, messageId: _quotedId),
      );
    });

    testWidgets('falls back to a fetched message', (tester) async {
      await pumpHeader(
        tester,
        fetched: (
          channelId: _channelId,
          message: _message(id: _quotedId, pubkey: _alice, content: 'Fetched'),
        ),
      );

      expect(find.text('Alice'), findsOneWidget);
      expect(find.text('Fetched'), findsOneWidget);
    });

    testWidgets('shows a fallback when the original is unavailable', (
      tester,
    ) async {
      await pumpHeader(
        tester,
        loadedMessages: [
          _message(
            id: _quotingId,
            pubkey: _bob,
            tags: [
              ['q', _quotedId, '', _alice],
            ],
          ),
        ],
      );

      expect(find.text('Original message unavailable'), findsOneWidget);
    });
  });
}

class _FixedUserCacheNotifier extends UserCacheNotifier {
  _FixedUserCacheNotifier(this._users);

  final Map<String, UserProfile> _users;

  @override
  Map<String, UserProfile> build() => _users;

  @override
  Future<bool> preload(List<String> pubkeys) async => true;
}
