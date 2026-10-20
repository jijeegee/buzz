import 'package:kuzz/features/home/home_page.dart';
import 'package:kuzz/features/channels/channels_page.dart';
import 'package:kuzz/features/profile/profile_avatar.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  Future<Widget> buildHome({
    int unreadInboxCount = 0,
    bool disableAnimations = false,
    Gradient? topSectionGradient,
  }) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    return ProviderScope(
      overrides: [savedPrefsProvider.overrideWithValue(prefs)],
      child: MaterialApp(
        theme: AppTheme.light(topSectionGradient: topSectionGradient),
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(
            context,
          ).copyWith(disableAnimations: disableAnimations),
          child: child!,
        ),
        home: HomePage(
          settingsPageBuilder: _buildSettingsPage,
          hasUnreadInbox: unreadInboxCount > 0,
        ),
      ),
    );
  }

  Future<void> openSettingsFromMore(WidgetTester tester) async {
    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();
    expect(find.byType(ProfileAvatar), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('more-profile')));
  }

  testWidgets('shows icon-only navigation and an aligned quick action', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pump();

    for (final label in ['People', 'Agents', 'More']) {
      expect(find.text(label), findsNothing);
      expect(find.bySemanticsLabel(label), findsOneWidget);
    }
    expect(find.byTooltip('Chats'), findsOneWidget);

    final quickAction = find.byTooltip('Create or start conversation');
    expect(quickAction, findsOneWidget);
    final launcherSize = tester.getSize(
      find.byType(ChannelQuickActionsLauncher),
    );
    expect(launcherSize.width, 800);
    expect(launcherSize.height, greaterThan(0));
    final motionRect = tester.getRect(
      find.byKey(const Key('channel-quick-actions-motion')),
    );
    expect(motionRect.width, const Size.square(56).width);
    expect(motionRect.left, greaterThanOrEqualTo(0));
    expect(tester.getSize(quickAction), const Size.square(56));
    final quickActionRect = tester.getRect(quickAction);
    expect(quickActionRect.left, greaterThanOrEqualTo(0));
    expect(quickActionRect.top, greaterThanOrEqualTo(0));
    expect(quickActionRect.right, lessThanOrEqualTo(800));
    expect(quickActionRect.bottom, lessThanOrEqualTo(600));
    final homeDestinationRect = tester.getRect(find.byTooltip('Chats'));
    expect(
      quickActionRect.center.dy,
      closeTo(homeDestinationRect.center.dy, 0.01),
    );
  });

  testWidgets('keeps a plain backdrop behind the scalable Home screen', (
    tester,
  ) async {
    const gradient = LinearGradient(
      begin: Alignment.topCenter,
      end: Alignment.bottomCenter,
      colors: [Colors.yellow, Colors.blue],
    );
    await tester.pumpWidget(await buildHome(topSectionGradient: gradient));
    await tester.pump();

    final backdrop = find.byKey(
      const ValueKey('home-settings-transition-backdrop'),
    );
    final decoration =
        tester.widget<DecoratedBox>(backdrop).decoration as BoxDecoration;
    // Chats has a plain header, so the channel list's gradient is not shown.
    expect(decoration.gradient, isNull);
    expect(
      find.byKey(const ValueKey('home-settings-transition-scale')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<Transform>(
            find.byKey(const ValueKey('home-settings-transition-scale')),
          )
          .transform
          .getMaxScaleOnAxis(),
      1,
    );
    expect(
      tester
          .widget<Opacity>(
            find.byKey(const ValueKey('home-settings-transition-opacity')),
          )
          .opacity,
      1,
    );
  });

  testWidgets('keeps Home opaque beneath the Settings transition', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pumpAndSettle();

    await openSettingsFromMore(tester);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 95));

    double homeOpacity() => tester
        .widget<Opacity>(
          find.byKey(const ValueKey('home-settings-transition-opacity')),
        )
        .opacity;

    expect(homeOpacity(), 1);

    await tester.pumpAndSettle();
    Navigator.of(
      tester.element(
        find.byKey(
          const ValueKey('settings-transition-opacity'),
          skipOffstage: false,
        ),
      ),
    ).pop();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 95));

    expect(homeOpacity(), 1);
  });

  testWidgets('uses one monotonic route animation for Settings and Home', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pumpAndSettle();

    double homeScale() => tester
        .widget<Transform>(
          find.byKey(const ValueKey('home-settings-transition-scale')),
        )
        .transform
        .storage[0];

    await openSettingsFromMore(tester);
    await tester.pump();

    final settingsTransition = find.byKey(
      const ValueKey('settings-transition-opacity'),
      skipOffstage: false,
    );
    final settingsRoute = ModalRoute.of(tester.element(settingsTransition));

    final entranceScales = <double>[homeScale()];
    final routeValues = <double>[settingsRoute!.animation!.value];
    for (var frame = 0; frame < 15; frame++) {
      await tester.pump(const Duration(milliseconds: 16));
      entranceScales.add(homeScale());
      routeValues.add(settingsRoute.animation!.value);
    }
    expect(entranceScales.first, closeTo(1, 0.000001));
    final reversalFrames = <int>[];
    for (var frame = 1; frame < entranceScales.length; frame++) {
      if (entranceScales[frame] > entranceScales[frame - 1] + 0.000001) {
        reversalFrames.add(frame);
      }
    }
    expect(
      reversalFrames,
      isEmpty,
      reason:
          'Home must scale down in one direction on entrance. '
          'scales=$entranceScales route=$routeValues',
    );
    expect(entranceScales, everyElement(inInclusiveRange(0.97, 1)));
    expect(entranceScales.last, closeTo(0.97, 0.001));

    await tester.pumpAndSettle();
    Navigator.of(tester.element(settingsTransition)).pop();
    await tester.pumpAndSettle();
  });

  testWidgets('gives selection haptics only when the tab changes', (
    tester,
  ) async {
    final hapticCalls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'HapticFeedback.vibrate') {
            hapticCalls.add(call);
          }
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
    );

    await tester.pumpWidget(await buildHome());
    await tester.pump();

    await tester.tap(find.byTooltip('Chats'));
    await tester.pump();
    expect(hapticCalls, isEmpty);

    await tester.tap(find.byTooltip('Agents'));
    await tester.pump();
    expect(hapticCalls, hasLength(1));
    expect(hapticCalls.single.arguments, 'HapticFeedbackType.selectionClick');

    await tester.tap(find.byTooltip('Agents'));
    await tester.pump();
    expect(hapticCalls, hasLength(1));

    await tester.tap(find.byTooltip('People'));
    await tester.pump();
    expect(hapticCalls, hasLength(2));
  });

  testWidgets('gives a light impact when the Home quick action is pressed', (
    tester,
  ) async {
    final hapticCalls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'HapticFeedback.vibrate') {
            hapticCalls.add(call);
          }
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
    );

    await tester.pumpWidget(await buildHome());
    await tester.pump();

    await tester.tap(find.byTooltip('Create or start conversation'));
    await tester.pump();

    expect(hapticCalls, hasLength(1));
    expect(hapticCalls.single.arguments, 'HapticFeedbackType.lightImpact');
  });

  testWidgets('hides the Activity tab even with unread rows', (tester) async {
    await tester.pumpWidget(await buildHome(unreadInboxCount: 1));
    await tester.pump();

    expect(find.byTooltip('Activity'), findsNothing);
    expect(find.byKey(const ValueKey('activity-tab-unread-dot')), findsNothing);
  });

  testWidgets('opens More as a small menu with profile and Settings only', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pump();

    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('more-menu')), findsOneWidget);
    expect(find.byKey(const ValueKey('more-settings')), findsOneWidget);
    for (final hidden in ['Groups', 'Search', 'Pulse']) {
      expect(find.text(hidden), findsNothing);
    }

    await tester.tapAt(const Offset(10, 10));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('more-menu')), findsNothing);
  });

  testWidgets('fades and slides tab content in the selected direction', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pump();

    Transform bodyTransform() => tester.widget<Transform>(
      find.byKey(const ValueKey('frosted-scaffold-body-transition-transform')),
    );
    Opacity bodyOpacity() => tester.widget<Opacity>(
      find.byKey(const ValueKey('frosted-scaffold-body-transition-opacity')),
    );
    Transform appBarTransform() => tester.widget<Transform>(
      find.byKey(
        const ValueKey('frosted-app-bar-content-transition-transform'),
      ),
    );
    Opacity appBarOpacity() => tester.widget<Opacity>(
      find.byKey(const ValueKey('frosted-app-bar-content-transition-opacity')),
    );
    double bodyOffset() => bodyTransform().transform.getTranslation().x;
    double appBarOffset() => appBarTransform().transform.getTranslation().x;

    expect(bodyOffset(), closeTo(0, 0.001));
    expect(appBarOffset(), closeTo(0, 0.001));
    expect(bodyOpacity().opacity, closeTo(1, 0.001));
    expect(appBarOpacity().opacity, closeTo(1, 0.001));

    await tester.tap(find.byTooltip('Agents'));
    await tester.pump();

    expect(bodyOffset(), closeTo(24, 0.001));
    expect(appBarOffset(), closeTo(24, 0.001));
    expect(bodyOpacity().opacity, closeTo(0, 0.001));
    expect(appBarOpacity().opacity, closeTo(0, 0.001));
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('frosted-app-bar-background')),
        matching: find.byKey(
          const ValueKey('frosted-app-bar-content-transition-transform'),
        ),
      ),
      findsOneWidget,
    );

    await tester.pump(const Duration(milliseconds: 120));

    expect(bodyOffset(), inExclusiveRange(0, 24));
    expect(appBarOffset(), inExclusiveRange(0, 24));
    expect(bodyOpacity().opacity, inExclusiveRange(0, 1));
    expect(appBarOpacity().opacity, inExclusiveRange(0, 1));

    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Chats'));
    await tester.pump();

    expect(bodyOffset(), closeTo(-24, 0.001));
    expect(appBarOffset(), closeTo(-24, 0.001));
    expect(bodyOpacity().opacity, closeTo(0, 0.001));
    expect(appBarOpacity().opacity, closeTo(0, 0.001));

    await tester.pumpAndSettle();
    expect(bodyOffset(), closeTo(0, 0.001));
    expect(appBarOffset(), closeTo(0, 0.001));
    expect(bodyOpacity().opacity, closeTo(1, 0.001));
    expect(appBarOpacity().opacity, closeTo(1, 0.001));
  });

  testWidgets('switches tab content instantly with reduced motion', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome(disableAnimations: true));
    await tester.pump();

    await tester.tap(find.byTooltip('Agents'));
    await tester.pump();

    final bodyTransform = tester.widget<Transform>(
      find.byKey(const ValueKey('frosted-scaffold-body-transition-transform')),
    );
    final bodyOpacity = tester.widget<Opacity>(
      find.byKey(const ValueKey('frosted-scaffold-body-transition-opacity')),
    );
    final appBarTransform = tester.widget<Transform>(
      find.byKey(
        const ValueKey('frosted-app-bar-content-transition-transform'),
      ),
    );
    final appBarOpacity = tester.widget<Opacity>(
      find.byKey(const ValueKey('frosted-app-bar-content-transition-opacity')),
    );
    expect(bodyTransform.transform.getTranslation().x, closeTo(0, 0.001));
    expect(appBarTransform.transform.getTranslation().x, closeTo(0, 0.001));
    expect(bodyOpacity.opacity, closeTo(1, 0.001));
    expect(appBarOpacity.opacity, closeTo(1, 0.001));
  });

  testWidgets('scales and fades the quick action as tabs change', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pump();

    double scale() => tester
        .widget<Transform>(find.byKey(const Key('channel-quick-actions-scale')))
        .transform
        .storage
        .first;
    double opacity() => tester
        .widget<Opacity>(find.byKey(const Key('channel-quick-actions-opacity')))
        .opacity;

    expect(scale(), closeTo(1, 0.001));
    expect(opacity(), closeTo(1, 0.001));

    await tester.tap(find.byTooltip('Agents'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 110));

    expect(scale(), inExclusiveRange(0.8, 1));
    expect(opacity(), inExclusiveRange(0, 1));

    await tester.pumpAndSettle();
    expect(scale(), closeTo(0.8, 0.001));
    expect(opacity(), closeTo(0, 0.001));

    await tester.tap(find.byTooltip('Chats'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 110));

    expect(scale(), inExclusiveRange(0.8, 1));
    expect(opacity(), inExclusiveRange(0, 1));

    await tester.pumpAndSettle();
    expect(scale(), closeTo(1, 0.001));
    expect(opacity(), closeTo(1, 0.001));
  });
}

Widget _buildSettingsPage(BuildContext context) => const SizedBox.shrink();
