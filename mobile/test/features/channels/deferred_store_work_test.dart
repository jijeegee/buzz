import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_preload.dart';
import 'package:kuzz/features/channels/deferred_store_work.dart';

class _NoRoomPreloader extends ChannelPreloader {
  _NoRoomPreloader(super.ref);

  @override
  bool get visibleRoomLoading => false;
}

void main() {
  testWidgets('store upkeep runs after the first frame, in order, even while '
      'a looping animation keeps the scheduler busy', (tester) async {
    final container = ProviderContainer(
      overrides: [channelPreloaderProvider.overrideWith(_NoRoomPreloader.new)],
    );
    addTearDown(container.dispose);
    await tester.pumpWidget(
      const MaterialApp(home: Center(child: CircularProgressIndicator())),
    );
    final ran = <int>[];
    final work = container.read(deferredStoreWorkProvider);
    work.run((_) => ran.add(1));
    work.run((_) => ran.add(2));
    expect(ran, isEmpty);
    for (var i = 0; i < 20 && ran.length < 2; i++) {
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(ran, [1, 2]);
    // Nothing is left pending.
    await tester.pumpWidget(const SizedBox());
  });
}
