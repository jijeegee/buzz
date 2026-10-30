part of '../channel_messages_provider.dart';

/// The account and server each room notifier last built for. The notifier
/// outlives an account switch (the family is kept), so it is told here.
final _builtScopes = Expando<({ChannelHeadScope? scope})>('builtScope');

/// A room notifier keeps nothing of another account: on an account or
/// server switch every row, window and pending request of the previous one
/// is dropped before this build draws, and the new account's saved rows are
/// looked up again.
extension _AccountScope on ChannelMessagesNotifier {
  /// Drops another account's state, then starts looking up this account's
  /// saved rows once.
  void _scopeToAccount() {
    _dropOtherAccount();
    if (_headRestoreStarted || _lastKnownMessages != null) return;
    final scope = _readHeadScope();
    if (scope != null) {
      _headRestoreStarted = true;
      unawaited(_restoreSavedRows(scope));
    } else {
      Future.microtask(_markSavedRowsChecked);
    }
  }

  void _dropOtherAccount() {
    final scope = _readHeadScope();
    final previous = _builtScopes[this];
    _builtScopes[this] = (scope: scope);
    if (previous == null || previous.scope == scope) return;
    // Retire every request of the previous account.
    _initVersion++;
    _windowGeneration++;
    _windowOpVersion++;
    _threadQuerySerial++;
    _clearSubscription();
    _initInFlight = false;
    _initialWindowQueryInFlight = false;
    _initFuture = null;
    _loadingNewer = false;
    _reachedOldest = false;
    _usingChannelWindow = false;
    _headRestoreStarted = false;
    _relayHistoryArrived = false;
    _cachedHeadIds = const {};
    _storedHead = null;
    _pendingRelayWindow = null;
    _paintedAnchor = null;
    _paintedContiguousFrom = 0;
    _followingStoredHead = false;
    final shown = _storedHeadShown;
    _storedHeadShown = null;
    if (shown != null && !shown.isCompleted) shown.complete();
    _windowStore = const ChannelWindowStore.empty();
    _lastKnownMessages = null;
    _liveSummaryRootsDuringInitialWindowQuery.clear();
    _deepLinkEvents.clear();
    _retainedDeepLinkEventIds.clear();
    _localReplyRoots.clear();
    _queryThreadSummaries.clear();
    _overflowFloors.clear();
    _threadQueryVersions.clear();
    _threadEvidenceVersions.clear();
    _deletionSummaryUncertainty.clear();
    _processedDeletionTargets.clear();
    _deferredDeletionTargets.clear();
    _summaryRefreshes.clear();
    _deletionOwnerQueue.clear();
    // The page waits for the new account's saved rows again.
    final checked = _read(channelSavedRowsCheckedProvider(channelId).notifier);
    Future.microtask(checked.reset);
  }
}
