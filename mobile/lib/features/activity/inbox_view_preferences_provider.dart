import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/theme/theme_provider.dart';
import 'inbox_item.dart';

const _filterPrefsKey = 'activity.inboxFilter';
const _unreadOnlyPrefsKey = 'activity.inboxUnreadOnly';

/// The Activity inbox's filter and unread-only toggle. Device-level view
/// preferences, so they survive leaving the tab and restarting the app
/// (desktop keeps the same two in local storage).
typedef InboxViewPreferences = ({InboxFilter filter, bool unreadOnly});

/// A stored filter name, or [InboxFilter.all] when missing or no longer known.
InboxFilter parseInboxFilter(String? name) =>
    InboxFilter.values.where((filter) => filter.name == name).firstOrNull ??
    InboxFilter.all;

class InboxViewPreferencesNotifier extends Notifier<InboxViewPreferences> {
  @override
  InboxViewPreferences build() {
    final prefs = ref.read(savedPrefsProvider);
    return (
      filter: parseInboxFilter(prefs.getString(_filterPrefsKey)),
      unreadOnly: prefs.getBool(_unreadOnlyPrefsKey) ?? false,
    );
  }

  void setFilter(InboxFilter filter) {
    state = (filter: filter, unreadOnly: state.unreadOnly);
    ref.read(savedPrefsProvider).setString(_filterPrefsKey, filter.name);
  }

  void setUnreadOnly(bool unreadOnly) {
    state = (filter: state.filter, unreadOnly: unreadOnly);
    ref.read(savedPrefsProvider).setBool(_unreadOnlyPrefsKey, unreadOnly);
  }
}

final inboxViewPreferencesProvider =
    NotifierProvider<InboxViewPreferencesNotifier, InboxViewPreferences>(
      InboxViewPreferencesNotifier.new,
    );
