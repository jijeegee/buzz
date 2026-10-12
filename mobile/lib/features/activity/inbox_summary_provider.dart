import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_state/read_state_provider.dart';
import '../../shared/relay/relay.dart';
import '../channels/channel_live_events.dart';
import '../channels/channels_provider.dart';
import 'inbox_summary.dart';

/// The Channels + Threads view from one relay summary: member rooms and their
/// replied-to threads, newest first, with server unread counts. The first page
/// renders at once; later pages load one after another in the background up
/// to [inboxSummaryBackgroundRows]. Live channel events patch only their row,
/// so a new message never refetches anything; reconnects, app resume, and
/// deletions of a shown message reload the first page. Mirrors desktop's
/// `useInboxSummary`.
///
/// An error with no value means the relay does not serve the summary (or the
/// first page failed twice); callers then fall back to per-room loading.
class InboxSummaryNotifier extends AsyncNotifier<InboxSummaryState> {
  int _generation = 0;

  /// Live events that arrive while a first page is in flight; folded into
  /// that page so a message racing the request is not lost.
  List<NostrEvent>? _eventsDuringLoad;

  @override
  Future<InboxSummaryState> build() async {
    ref.watch(relayConfigProvider);
    ref.watch(myPubkeyProvider);
    final connected = ref.watch(
      relaySessionProvider.select((s) => s.status == SessionStatus.connected),
    );
    final generation = ++_generation;
    ref.onDispose(() => _generation++);
    // Keep the last rows on screen until the session reconnects, which
    // rebuilds this provider and reloads the summary.
    if (!connected) return Completer<InboxSummaryState>().future;

    ref.onDispose(
      ref
          .read(channelLiveEventsProvider)
          .listen((event) => _handleLiveEvent(event, generation)),
    );
    // Messages missed while backgrounded never reached the row patcher.
    ref.listen(appLifecycleProvider, (previous, next) {
      if (previous != AppLifecycleState.resumed &&
          next == AppLifecycleState.resumed) {
        _reloadIfCurrent(generation);
      }
    });
    // Read state loads after startup; count unread once it is available.
    ref.listen(readStateProvider.select((s) => s.isReady), (_, ready) {
      if (ready && state.value?.sentReadMarkers == false) {
        _reloadIfCurrent(generation);
      }
    });

    final read = _readContexts();
    final buffered = _eventsDuringLoad = [];
    final InboxSummaryPage page;
    try {
      page = await _fetchPage(null, read);
    } finally {
      if (identical(_eventsDuringLoad, buffered)) _eventsDuringLoad = null;
    }
    var summary = InboxSummaryState(
      rows: page.rows,
      next: page.next,
      sentRead: read,
    );
    var reload = false;
    for (final event in buffered) {
      final result = _foldLiveEvent(summary, event);
      summary = result.state;
      reload = reload || result.reload;
    }
    // Read state became ready while this page was in flight.
    if (read.isEmpty && _readContexts().isNotEmpty) reload = true;
    if (reload) {
      Timer.run(() => _reloadIfCurrent(generation));
    } else if (summary.next != null) {
      // After this build has published its first page.
      Timer.run(() => unawaited(_loadMorePages(generation)));
    }
    return summary;
  }

  Map<String, int> _readContexts() {
    final readState = ref.read(readStateProvider);
    return readState.isReady
        ? inboxSummaryReadContexts(readState.contexts)
        : const {};
  }

  Future<InboxSummaryPage> _fetchPage(
    InboxSummaryCursor? before,
    Map<String, int> read,
  ) async {
    final response = await ref
        .read(relaySessionProvider.notifier)
        .queryRelayJson([
          buildInboxSummaryFilter(
            limit: inboxSummaryPageSize,
            before: before,
            read: read,
          ),
        ]);
    return parseInboxSummaryResponse(response);
  }

  Future<void> _loadMorePages(int generation) async {
    while (generation == _generation && ref.mounted) {
      final current = state.value;
      final next = current?.next;
      if (current == null ||
          next == null ||
          current.rows.length >= inboxSummaryBackgroundRows) {
        return;
      }
      final InboxSummaryPage page;
      try {
        // Later pages count against the same markers as the first page.
        page = await _fetchPage(next, current.sentRead);
      } catch (error) {
        // The next reload (reconnect, resume, deletion) restarts paging.
        debugPrint('[InboxSummary] page failed: $error');
        return;
      }
      if (generation != _generation || !ref.mounted) return;
      final latest = state.value;
      // A newer first page or another page landed meanwhile.
      if (latest == null || latest.next != next) return;
      state = AsyncData(mergeInboxSummaryPage(latest, page));
    }
  }

  void _handleLiveEvent(NostrEvent event, int generation) {
    if (generation != _generation) return;
    final buffered = _eventsDuringLoad;
    if (buffered != null) {
      buffered.add(event);
      return;
    }
    final current = state.value;
    if (current == null) return;
    final result = _foldLiveEvent(current, event);
    if (result.reload) {
      _reloadIfCurrent(generation);
    } else if (!identical(result.state, current)) {
      state = AsyncData(result.state);
    }
  }

  /// Patches the row [event] belongs to. A deletion of a shown message or
  /// thread asks for a reload instead; other deletions (for example removed
  /// reactions) change no row.
  ({InboxSummaryState state, bool reload}) _foldLiveEvent(
    InboxSummaryState current,
    NostrEvent event,
  ) {
    final channelId = event.channelId;
    if (channelId == null) return (state: current, reload: false);
    if (event.kind == EventKind.deletion ||
        event.kind == EventKind.nip29DeleteEvent) {
      final targets = [
        for (final tag in event.tags)
          if (tag.length > 1 && tag[0] == 'e') tag[1],
      ];
      return (state: current, reload: inboxSummaryShowsAny(current, targets));
    }
    final channel = ref
        .read(channelsProvider)
        .asData
        ?.value
        .where((channel) => channel.id == channelId)
        .firstOrNull;
    return (
      state: applyLiveEventToInboxSummary(
        current,
        event,
        channelId: channelId,
        channelType: channel?.isDm == true ? 'dm' : 'stream',
        currentPubkey: ref.read(myPubkeyProvider),
      ),
      reload: false,
    );
  }

  void _reloadIfCurrent(int generation) {
    if (generation != _generation || !ref.mounted) return;
    if (ref.read(relaySessionProvider).status != SessionStatus.connected) {
      return;
    }
    ref.invalidateSelf();
  }

  /// Reloads the first page (pull to refresh).
  Future<void> refresh() async {
    ref.invalidateSelf();
    try {
      await future;
    } catch (error) {
      // The failed state itself switches the view to per-room loading.
      debugPrint('[InboxSummary] refresh failed: $error');
    }
  }
}

/// One retry for a transient failure, as desktop does; none when the relay
/// does not serve the summary or a statement deadline would repeat.
Duration? _inboxSummaryRetry(int retryCount, Object error) {
  if (retryCount > 0 ||
      error is InboxSummaryUnsupportedException ||
      isRelayDeadlineError(error)) {
    return null;
  }
  return const Duration(seconds: 1);
}

/// Loaded only while the Channels + Threads view is open.
final inboxSummaryProvider =
    AsyncNotifierProvider.autoDispose<InboxSummaryNotifier, InboxSummaryState>(
      InboxSummaryNotifier.new,
      retry: _inboxSummaryRetry,
    );

/// Whether the Channels + Threads view must use per-room loading: the relay
/// does not serve the summary, or the first page failed for good.
bool inboxSummaryFailed(AsyncValue<InboxSummaryState> summary) =>
    summary.hasError && !summary.hasValue && !summary.retrying;

/// Server unread counts keyed by conversation id, empty until loaded.
final inboxSummaryUnreadProvider =
    Provider.autoDispose<Map<String, InboxServerUnread>>((ref) {
      final summary = ref.watch(inboxSummaryProvider).value;
      return summary == null ? const {} : inboxSummaryUnread(summary);
    });
