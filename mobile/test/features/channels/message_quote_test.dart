import 'dart:async';

import 'package:kuzz/features/channels/channel_identity_names_provider.dart';
import 'package:kuzz/features/people/blocked_message_gate.dart';
import 'package:kuzz/features/people/blocked_users_provider.dart';
import 'package:kuzz/features/channels/composer_quote_chip.dart';
import 'package:kuzz/features/channels/composer_quote_provider.dart';
import 'package:kuzz/features/channels/message_quote.dart';
import 'package:kuzz/features/channels/message_quote_header.dart';
import 'package:kuzz/features/channels/quote_preview_provider.dart';
import 'package:kuzz/features/channels/quoted_message_provider.dart';
import 'package:kuzz/features/channels/timeline_message.dart';
import 'package:kuzz/shared/deeplink/deep_link.dart';
import 'package:kuzz/shared/deeplink/pending_deep_link_provider.dart';
import 'package:kuzz/shared/identity_names/identity_names.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
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

    late List<QuotePreviewKey> previewRequests;

    List<Override> overrides({
      QuotedMessage? fetched,
      QuotePreview? preview,
      Set<String> blocked = const {},
      bool? foldsBlocked,
    }) => [
      blockedPubkeysProvider.overrideWithValue(blocked),
      if (foldsBlocked != null)
        foldsBlockedAuthorsProvider(_channelId).overrideWithValue(foldsBlocked),
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
      quotePreviewProvider.overrideWith((ref, key) async {
        previewRequests.add(key);
        return preview;
      }),
    ];

    Future<ProviderContainer> pumpHeader(
      WidgetTester tester, {
      List<TimelineMessage>? loadedMessages,
      QuotedMessage? fetched,
      List<List<String>>? tags,
      QuoteJumpScope Function(Widget child)? wrap,
      Set<String> blocked = const {},
      bool? foldsBlocked,
      QuotePreview? preview,
      String? quotingMessageId,
    }) async {
      previewRequests = [];
      final header = MessageQuoteHeader(
        channelId: _channelId,
        tags:
            tags ??
            [
              ['h', _channelId],
              ['q', _quotedId, '', _alice],
            ],
        loadedMessages: loadedMessages,
        quotingMessageId: quotingMessageId,
      );
      await tester.pumpWidget(
        WidgetHelpers.testable(
          overrides: overrides(
            fetched: fetched,
            preview: preview,
            blocked: blocked,
            foldsBlocked: foldsBlocked,
          ),
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

    testWidgets('folds a quote of a blocked author', (tester) async {
      await pumpHeader(
        tester,
        blocked: {_alice.toLowerCase()},
        loadedMessages: [
          _message(id: _quotedId, pubkey: _alice, content: 'Secret text'),
        ],
      );
      expect(find.text('Message from a blocked user'), findsOneWidget);
      expect(find.text('Secret text'), findsNothing);
      expect(find.text('Alice'), findsNothing);
    });

    testWidgets('shows a quote of a blocked author in a one-to-one DM', (
      tester,
    ) async {
      await pumpHeader(
        tester,
        blocked: {_alice.toLowerCase()},
        foldsBlocked: false,
        loadedMessages: [
          _message(id: _quotedId, pubkey: _alice, content: 'Before the block'),
        ],
      );
      expect(find.text('Message from a blocked user'), findsNothing);
      expect(find.text('Before the block'), findsOneWidget);
      expect(find.text('Alice'), findsOneWidget);
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
      expect(previewRequests, isEmpty);
    });

    testWidgets('shows the relay preview of a message sent before I joined', (
      tester,
    ) async {
      final container = await pumpHeader(
        tester,
        quotingMessageId: _quotingId,
        preview: QuotePreview(authorPubkey: _alice, excerpt: 'Old **news**'),
      );

      expect(previewRequests, [
        (channelId: _channelId, quotedId: _quotedId, quotingId: _quotingId),
      ]);
      final box = find.byKey(ValueKey('message-quote-preview-$_quotedId'));
      expect(box, findsOneWidget);
      expect(find.textContaining('Alice'), findsOneWidget);
      expect(find.textContaining('Old news'), findsOneWidget);
      expect(find.text('Sent before you joined'), findsOneWidget);
      // Not a link: the original stays hidden.
      expect(find.byType(InkWell), findsNothing);
      await tester.tap(box);
      await tester.pump();
      expect(container.read(pendingDeepLinkProvider), isNull);
    });

    testWidgets('keeps the fallback when the relay has no preview', (
      tester,
    ) async {
      await pumpHeader(tester, quotingMessageId: _quotingId);

      expect(previewRequests, hasLength(1));
      expect(find.text('Original message unavailable'), findsOneWidget);
      expect(find.text('Sent before you joined'), findsNothing);
    });

    testWidgets('folds a preview of a blocked author', (tester) async {
      await pumpHeader(
        tester,
        quotingMessageId: _quotingId,
        blocked: {_alice},
        preview: QuotePreview(authorPubkey: _alice, excerpt: 'Secret'),
      );
      expect(find.text('Message from a blocked user'), findsOneWidget);
      expect(find.textContaining('Secret'), findsNothing);
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
