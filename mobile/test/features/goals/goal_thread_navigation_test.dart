import 'package:kuzz/features/goals/goal_thread_navigation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

MaterialPageRoute<void> _page(String name) =>
    MaterialPageRoute<void>(builder: (_) => Scaffold(body: Text(name)));

void main() {
  late GlobalKey<NavigatorState> key;

  Future<NavigatorState> stack(
    WidgetTester tester,
    List<Route<void>> routes,
  ) async {
    key = GlobalKey<NavigatorState>();
    await tester.pumpWidget(
      MaterialApp(
        navigatorKey: key,
        home: const Scaffold(body: Text('home')),
      ),
    );
    for (final route in routes) {
      key.currentState!.push(route);
      await tester.pumpAndSettle();
    }
    return key.currentState!;
  }

  Future<void> back(WidgetTester tester) async {
    key.currentState!.pop();
    await tester.pumpAndSettle();
  }

  testWidgets('a message replaces the open channel; Back leaves the channel', (
    tester,
  ) async {
    final channel = _page('channel');
    final navigator = await stack(tester, [
      channel,
      _page('thread'),
      _page('goals'),
    ]);
    showGoalLinkRoute(
      navigator,
      _page('jumped'),
      channelRoute: channel,
      replaceCurrent: true,
    );
    await tester.pumpAndSettle();
    expect(find.text('jumped'), findsOneWidget);
    await back(tester);
    expect(find.text('home'), findsOneWidget);
  });

  testWidgets('without an open channel the link replaces the thread', (
    tester,
  ) async {
    final navigator = await stack(tester, [
      _page('channel'),
      _page('thread'),
      _page('goals'),
    ]);
    showGoalLinkRoute(
      navigator,
      _page('other thread'),
      channelRoute: null,
      replaceCurrent: true,
    );
    await tester.pumpAndSettle();
    expect(find.text('other thread'), findsOneWidget);
    await back(tester);
    expect(find.text('channel'), findsOneWidget);
  });
}
