import 'dart:async';

import 'package:kuzz/shared/widgets/kuzz_mark.dart';
import 'package:kuzz/shared/widgets/kuzz_refresh_indicator.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../helpers/widget_helpers.dart';

void main() {
  testWidgets('shows the mark while pulling to refresh', (tester) async {
    const contentKey = ValueKey('loading-content');
    var refreshes = 0;
    final refreshCompleter = Completer<void>();

    await tester.pumpWidget(
      WidgetHelpers.testable(
        child: KuzzRefreshIndicator(
          onRefresh: () {
            refreshes++;
            return refreshCompleter.future;
          },
          child: ListView(
            children: const [SizedBox(key: contentKey, height: 800)],
          ),
        ),
      ),
    );

    final listFinder = find.byType(ListView);
    final restingTop = tester.getTopLeft(listFinder).dy;
    final restingContentTop = tester.getTopLeft(find.byKey(contentKey)).dy;
    await tester.timedDrag(
      listFinder,
      const Offset(0, 320),
      const Duration(milliseconds: 500),
    );
    await tester.pump(const Duration(milliseconds: 16));
    await tester.pump(const Duration(milliseconds: 300));

    final markFinder = find.byType(KuzzLoadingMark);
    final loadingTop = tester.getTopLeft(listFinder).dy;
    final loadingContentTop = tester.getTopLeft(find.byKey(contentKey)).dy;
    final gapTransform = tester.widget<Transform>(
      find.byKey(const ValueKey('kuzz-refresh-retained-gap')),
    );
    expect(markFinder, findsOneWidget);
    expect(tester.widget<KuzzLoadingMark>(markFinder).active, isTrue);
    expect(refreshes, 1);
    expect(gapTransform.transform.getTranslation().y, closeTo(72, 1));
    expect(loadingTop - restingTop, closeTo(72, 1));
    final pulse = tester.widget<ScaleTransition>(
      find.descendant(of: markFinder, matching: find.byType(ScaleTransition)),
    );
    final initialScale = pulse.scale.value;
    await tester.pump(const Duration(milliseconds: 120));
    expect(pulse.scale.value, isNot(initialScale));

    refreshCompleter.complete();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 90));

    final closingTop = tester.getTopLeft(listFinder).dy;
    expect(closingTop, greaterThan(restingTop));
    expect(closingTop, lessThan(loadingTop));
    expect(loadingContentTop, greaterThan(restingContentTop));

    await tester.pumpAndSettle();
    expect(tester.getTopLeft(listFinder).dy, closeTo(restingTop, 1));
    expect(markFinder, findsNothing);
  });

  testWidgets('tracks each stage of an active pull', (tester) async {
    tester.view.physicalSize = const Size(420, 912);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final hapticCalls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'HapticFeedback.vibrate') hapticCalls.add(call);
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
    );

    const contentKey = ValueKey('pull-content');
    final refreshCompleter = Completer<void>();
    await tester.pumpWidget(
      WidgetHelpers.testable(
        child: KuzzRefreshIndicator(
          onRefresh: () => refreshCompleter.future,
          child: ListView(
            children: const [SizedBox(key: contentKey, height: 800)],
          ),
        ),
      ),
    );

    final restingContentTop = tester.getTopLeft(find.byKey(contentKey)).dy;
    final gesture = await tester.startGesture(
      tester.getCenter(find.byType(ListView)),
      pointer: 1,
    );
    await gesture.moveBy(const Offset(0, 12));
    await tester.pump();

    final markFinder = find.byType(KuzzLoadingMark);
    expect(markFinder, findsNothing);
    expect(
      tester.getTopLeft(find.byKey(contentKey)).dy,
      greaterThan(restingContentTop),
    );

    await gesture.moveBy(const Offset(0, 44));
    await tester.pump();

    final earlyTop = tester.getTopLeft(markFinder).dy;
    final partialOpacity = tester.widget<Opacity>(
      find.byKey(const ValueKey('kuzz-refresh-opacity')),
    );
    expect(partialOpacity.opacity, greaterThan(0));
    expect(partialOpacity.opacity, lessThan(1));
    final partialScale = tester
        .widget<Transform>(find.byKey(const ValueKey('kuzz-refresh-scale')))
        .transform
        .storage[0];
    expect(partialScale, greaterThan(0.6));
    expect(partialScale, lessThan(1));
    expect(tester.widget<KuzzLoadingMark>(markFinder).active, isFalse);
    expect(hapticCalls, isEmpty);

    await gesture.moveBy(const Offset(0, 120));
    await tester.pump();

    final pulledContentTop = tester.getTopLeft(find.byKey(contentKey)).dy;
    expect(tester.getTopLeft(markFinder).dy, greaterThan(earlyTop));
    final pulledRect = tester.getRect(markFinder);
    final pulledGap = pulledContentTop - restingContentTop;
    expect(
      pulledRect.center.dy,
      closeTo(
        restingContentTop +
            (pulledGap - pulledRect.height) * 0.75 +
            pulledRect.height / 2,
        1,
      ),
    );

    await gesture.moveBy(const Offset(0, 120));
    await tester.pump();

    expect(
      tester
          .widget<Transform>(find.byKey(const ValueKey('kuzz-refresh-scale')))
          .transform
          .storage[0],
      closeTo(1, 0.001),
    );
    expect(hapticCalls, hasLength(1));
    expect(hapticCalls.single.arguments, 'HapticFeedbackType.mediumImpact');

    await gesture.moveBy(const Offset(0, 500));
    await tester.pump();
    expect(hapticCalls, hasLength(1));

    await gesture.up();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(hapticCalls, hasLength(1));

    refreshCompleter.complete();
    await tester.pumpAndSettle();
  });

  testWidgets('keeps the mark static when motion is disabled', (tester) async {
    await tester.pumpWidget(
      MediaQuery(
        data: const MediaQueryData(disableAnimations: true),
        child: WidgetHelpers.testable(
          child: Builder(
            builder: (context) => MediaQuery(
              data: MediaQuery.of(context).copyWith(disableAnimations: true),
              child: KuzzRefreshIndicator(
                onRefresh: () async {},
                child: ListView(children: const [SizedBox(height: 800)]),
              ),
            ),
          ),
        ),
      ),
    );

    await tester.timedDrag(
      find.byType(ListView),
      const Offset(0, 160),
      const Duration(milliseconds: 400),
    );
    await tester.pump();

    final mark = find.byType(KuzzLoadingMark);
    expect(find.byType(KuzzMark), findsOneWidget);
    expect(
      find.descendant(of: mark, matching: find.byType(ScaleTransition)),
      findsNothing,
    );
  });

  testWidgets('provides elastic always-scrollable physics', (tester) async {
    late ScrollPhysics physics;

    await tester.pumpWidget(
      WidgetHelpers.testable(
        child: KuzzRefreshIndicator(
          onRefresh: () async {},
          child: Builder(
            builder: (context) {
              physics = ScrollConfiguration.of(
                context,
              ).getScrollPhysics(context);
              return ListView(children: const [SizedBox(height: 20)]);
            },
          ),
        ),
      ),
    );

    expect(physics, isA<BouncingScrollPhysics>());
    expect(physics.parent, isA<AlwaysScrollableScrollPhysics>());
  });
}
