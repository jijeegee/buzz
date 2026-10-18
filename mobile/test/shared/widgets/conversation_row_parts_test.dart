import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:kuzz/shared/widgets/conversation_row_parts.dart';

final _mina = 'a' * 64;
final _jun = 'b' * 64;

void main() {
  group('resolvePreviewMentions', () {
    test('a qualified mention reads the cached name', () {
      expect(
        resolvePreviewMentions(
          'hi @Old Name ($_mina) and thanks',
          [
            ['p', _mina],
          ],
          {_mina: 'Mina'},
        ),
        'hi @Mina and thanks',
      );
    });

    test('without a cached name the pubkey still goes', () {
      expect(
        resolvePreviewMentions('hi @Old Name ($_mina)', [
          ['p', _mina],
        ], const {}),
        'hi @Old Name',
      );
    });

    test('an untagged pubkey never borrows a cached name', () {
      expect(
        resolvePreviewMentions('hi @Someone ($_jun)', const [], {_jun: 'Jun'}),
        'hi @Someone',
      );
    });

    test('plain text is left as it is', () {
      expect(
        resolvePreviewMentions(
          'ask @Mina later',
          [
            ['p', _mina],
          ],
          {_mina: 'Mina'},
        ),
        'ask @Mina later',
      );
    });
  });

  test('conversationTimestamp: time today, Yesterday, then a date', () {
    final now = DateTime(2026, 10, 18, 15, 30);
    int at(DateTime date) => date.millisecondsSinceEpoch ~/ 1000;
    expect(
      conversationTimestamp(at(DateTime(2026, 10, 18, 9, 5)), now: now),
      '9:05 AM',
    );
    expect(
      conversationTimestamp(at(DateTime(2026, 10, 17, 23)), now: now),
      'Yesterday',
    );
    expect(conversationTimestamp(at(DateTime(2026, 10, 2)), now: now), 'Oct 2');
    expect(
      conversationTimestamp(at(DateTime(2025, 1, 2)), now: now),
      'Jan 2, 2025',
    );
  });

  testWidgets('the list background is white in a light theme', (tester) async {
    late Color light;
    late Color dark;
    await tester.pumpWidget(
      Theme(
        data: AppTheme.light(),
        child: Builder(
          builder: (context) {
            light = conversationListBackground(context);
            return const SizedBox();
          },
        ),
      ),
    );
    await tester.pumpWidget(
      Theme(
        data: AppTheme.dark(),
        child: Builder(
          builder: (context) {
            dark = conversationListBackground(context);
            return const SizedBox();
          },
        ),
      ),
    );
    expect(light, Colors.white);
    expect(dark, AppTheme.dark().colorScheme.surface);
  });

  testWidgets('a two-circle avatar keeps the avatar column width', (
    tester,
  ) async {
    await tester.pumpWidget(
      const Directionality(
        textDirection: TextDirection.ltr,
        child: Center(
          child: ConversationAvatarPair(
            key: ValueKey('pair'),
            back: SizedBox.expand(key: ValueKey('back')),
            front: SizedBox.expand(key: ValueKey('front')),
            ringColor: Colors.white,
          ),
        ),
      ),
    );
    final pair = tester.getRect(find.byKey(const ValueKey('pair')));
    final back = tester.getRect(find.byKey(const ValueKey('back')));
    final front = tester.getRect(find.byKey(const ValueKey('front')));
    expect(pair.size, const Size(activityAvatarSize, activityAvatarSize));
    expect(back.size, const Size.square(conversationPairCircleSize));
    expect(front.size, const Size.square(conversationPairCircleSize));
    // Centred on the avatar column.
    expect(back.left, pair.left - conversationPairOverhang);
    expect(front.right, pair.right + conversationPairOverhang);
    expect(front.right - back.left, conversationPairWidth);
    expect(front.center.dy, back.center.dy);
  });

  group('ConversationRowLayout', () {
    Widget host(Widget row, {double textScale = 1}) => MaterialApp(
      home: MediaQuery(
        data: MediaQueryData(
          size: const Size(400, 800),
          textScaler: TextScaler.linear(textScale),
        ),
        child: Scaffold(
          body: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(width: 400, child: row),
          ),
        ),
      ),
    );

    testWidgets('keeps the Activity row skeleton', (tester) async {
      await tester.pumpWidget(
        host(
          const ConversationRowLayout(
            avatar: SizedBox.square(
              key: ValueKey('avatar'),
              dimension: activityAvatarSize,
            ),
            title: Text('Room', key: ValueKey('title')),
            titleTrailing: [Text('3:05 PM', key: ValueKey('time'))],
            preview: Text('Hello', key: ValueKey('preview')),
          ),
        ),
      );
      final avatar = tester.getRect(find.byKey(const ValueKey('avatar')));
      final title = tester.getRect(find.byKey(const ValueKey('title')));
      final time = tester.getRect(find.byKey(const ValueKey('time')));
      final preview = tester.getRect(find.byKey(const ValueKey('preview')));
      expect(avatar.topLeft, const Offset(Grid.gutter, Grid.twelve));
      expect(title.left, avatar.right + messageAvatarContentGap);
      expect(time.right, 400 - Grid.gutter);
      expect(preview.left, title.left);
      expect(preview.top, title.bottom + conversationTitlePreviewGap);
    });

    testWidgets('a side column keeps the preview clear of it', (tester) async {
      await tester.pumpWidget(
        host(
          ConversationRowLayout(
            startInset: 25,
            avatar: const SizedBox.square(dimension: activityAvatarSize),
            title: const Text('Room', key: ValueKey('title')),
            preview: Text(
              'word ' * 60,
              key: const ValueKey('preview'),
              maxLines: 2,
            ),
            side: const Text('3:05 PM', key: ValueKey('side')),
          ),
        ),
      );
      final title = tester.getRect(find.byKey(const ValueKey('title')));
      final preview = tester.getRect(find.byKey(const ValueKey('preview')));
      final side = tester.getRect(find.byKey(const ValueKey('side')));
      expect(title.left, Grid.gutter + 25 + activityAvatarSize + Grid.xxs);
      expect(side.top, title.top);
      expect(side.right, 400 - Grid.gutter);
      expect(preview.right, lessThanOrEqualTo(side.left - Grid.xxs));
    });

    testWidgets('the title line grows with the text size', (tester) async {
      late double normal;
      late double large;
      await tester.pumpWidget(
        host(
          Builder(
            builder: (context) {
              normal = conversationTitleLineHeight(context);
              return const SizedBox();
            },
          ),
        ),
      );
      await tester.pumpWidget(
        host(
          Builder(
            builder: (context) {
              large = conversationTitleLineHeight(context);
              return const SizedBox();
            },
          ),
          textScale: 1.5,
        ),
      );
      expect(normal, closeTo(17, 0.01));
      expect(large, closeTo(25.5, 0.01));
    });
  });
}
