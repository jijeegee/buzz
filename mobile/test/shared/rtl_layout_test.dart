import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/chat_bubble_row.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:kuzz/shared/widgets/app_list.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

const _width = 400.0;

Future<void> _pump(
  WidgetTester tester,
  TextDirection direction,
  Widget child,
) async {
  await tester.pumpWidget(
    MaterialApp(
      theme: AppTheme.light(),
      home: Directionality(
        textDirection: direction,
        child: Scaffold(
          body: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(width: _width, child: child),
          ),
        ),
      ),
    ),
  );
}

Widget _bubble({required bool isOwn}) => ChatBubbleRow(
  isOwn: isOwn,
  leading: const SizedBox(key: Key('avatar'), width: 32, height: 32),
  showTime: true,
  createdAt: 1700000000,
  bubbleKey: const Key('bubble'),
  timestampKey: const Key('time'),
  child: const Text('hi'),
);

void main() {
  for (final direction in TextDirection.values) {
    final rtl = direction == TextDirection.rtl;

    testWidgets('own bubble sits on the ${rtl ? 'left' : 'right'} in '
        '${direction.name}, tail on its outer bottom corner', (tester) async {
      await _pump(tester, direction, _bubble(isOwn: true));

      final bubble = tester.getRect(find.byKey(const Key('bubble')));
      final time = tester.getRect(find.byKey(const Key('time')));
      expect(bubble.center.dx < _width / 2, rtl);
      // The time sits on the bubble's inner side.
      expect(time.center.dx > bubble.center.dx, rtl);

      final decoration =
          tester
                  .widget<DecoratedBox>(find.byKey(const Key('bubble')))
                  .decoration
              as BoxDecoration;
      final radius = decoration.borderRadius!.resolve(direction);
      final tail = rtl ? radius.bottomLeft : radius.bottomRight;
      final round = rtl ? radius.bottomRight : radius.bottomLeft;
      expect(tail.x, lessThan(round.x));
    });

    testWidgets('other bubble keeps its avatar on the '
        '${rtl ? 'right' : 'left'} in ${direction.name}', (tester) async {
      await _pump(tester, direction, _bubble(isOwn: false));

      final avatar = tester.getRect(find.byKey(const Key('avatar')));
      final bubble = tester.getRect(find.byKey(const Key('bubble')));
      expect(rtl ? avatar.right : avatar.left, rtl ? _width : 0);
      expect(avatar.center.dx > bubble.center.dx, rtl);
    });

    testWidgets('settings row mirrors icon and chevron in ${direction.name}', (
      tester,
    ) async {
      await _pump(
        tester,
        direction,
        const AppListRow(
          icon: LucideIcons.bell,
          title: 'Notifications',
          trailing: Icon(LucideIcons.chevronRightDir),
        ),
      );

      final leading = tester.getCenter(find.byIcon(LucideIcons.bell));
      final title = tester.getCenter(find.text('Notifications'));
      final chevron = tester.getCenter(
        find.byIcon(LucideIcons.chevronRightDir),
      );
      expect(leading.dx > title.dx, rtl);
      expect(chevron.dx < title.dx, rtl);
      // The chevron points forward: flipped to point left in RTL.
      expect(
        find.descendant(
          of: find.byIcon(LucideIcons.chevronRightDir),
          matching: find.byType(Transform),
        ),
        rtl ? findsOneWidget : findsNothing,
      );
    });
  }
}
