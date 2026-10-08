import 'package:buzz/features/activity/inbox_item.dart';
import 'package:buzz/features/activity/inbox_view_preferences_provider.dart';
import 'package:buzz/shared/theme/theme_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

Future<ProviderContainer> containerWith(Map<String, Object> stored) async {
  SharedPreferences.setMockInitialValues(stored);
  final prefs = await SharedPreferences.getInstance();
  final container = ProviderContainer(
    overrides: [savedPrefsProvider.overrideWithValue(prefs)],
  );
  addTearDown(container.dispose);
  return container;
}

void main() {
  test('defaults to All with unread-only off', () async {
    final container = await containerWith({});
    final prefs = container.read(inboxViewPreferencesProvider);
    expect(prefs.filter, InboxFilter.all);
    expect(prefs.unreadOnly, isFalse);
  });

  test('a chosen filter and unread-only survive a restart', () async {
    final first = await containerWith({});
    first
        .read(inboxViewPreferencesProvider.notifier)
        .setFilter(InboxFilter.conversations);
    first.read(inboxViewPreferencesProvider.notifier).setUnreadOnly(true);

    // A new container over the same stored values stands in for a restart.
    final prefs = await SharedPreferences.getInstance();
    final restarted = ProviderContainer(
      overrides: [savedPrefsProvider.overrideWithValue(prefs)],
    );
    addTearDown(restarted.dispose);
    final restored = restarted.read(inboxViewPreferencesProvider);
    expect(restored.filter, InboxFilter.conversations);
    expect(restored.unreadOnly, isTrue);
  });

  test('an unknown stored filter falls back to All', () {
    expect(parseInboxFilter('retired'), InboxFilter.all);
    expect(parseInboxFilter(null), InboxFilter.all);
    expect(parseInboxFilter('thread'), InboxFilter.thread);
  });
}
