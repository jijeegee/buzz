part of 'channels_provider.dart';

/// How long the relay's list waits before it is saved, so live updates in a
/// burst write once.
const _channelListSaveDelay = Duration(seconds: 1);

/// The channel list saved in the account's store file, drawn at launch
/// before the token, the socket and the relay's list
/// (docs/message-sync.md#launch-order). The saved list is only drawn: it
/// never counts as the relay's list ([ChannelsNotifier.hasLoaded]) and never
/// keeps a room from being forgotten by the left-room purge.
class _ChannelListSnapshot {
  _ChannelListSnapshot(this._notifier);

  final ChannelsNotifier _notifier;

  /// Whether the list shown is the saved one, not yet replaced by the
  /// relay's.
  bool shown = false;

  int _paint = 0;

  /// The read of the saved list in flight, so a relay failure that beats it
  /// still keeps the saved list and retries.
  Future<void> pending = Future<void>.value();
  Timer? _saveTimer;
  List<Channel>? _saved;

  Ref get _ref => _notifier._lifecycleRef;

  MessageStoreLease? _lease(String relayBaseUrl, String? pubkey) =>
      _ref.read(messageStoreProvider).lease(relayBaseUrl, pubkey);

  /// A new relay or account: nothing of the previous one is saved or drawn.
  void resetScope() {
    _paint++;
    _saveTimer?.cancel();
    _saveTimer = null;
    _saved = null;
    shown = false;
  }

  /// Draws the saved list of [relayBaseUrl] and [pubkey] unless the relay's
  /// list, or another scope, got there first.
  void paint(String relayBaseUrl, String? pubkey) {
    final paint = ++_paint;
    final lease = _lease(relayBaseUrl, pubkey);
    if (lease == null) return;
    pending = _ref
        .read(messageStoreProvider)
        .readSnapshot(lease, channelListSnapshotKey)
        .then((raw) {
          final channels = decodeChannelListSnapshot(raw);
          if (channels == null ||
              paint != _paint ||
              !_ref.mounted ||
              _notifier._hasLoaded ||
              _notifier._memberSnapshotRelayBaseUrl != relayBaseUrl ||
              _notifier._memberSnapshotPubkey != pubkey) {
            return;
          }
          shown = true;
          _saved = channels;
          _notifier._showList(channels);
        })
        .catchError((Object _) {});
  }

  /// Saves the relay's list (and the live updates on it) shortly after it
  /// is shown.
  void listShown(AsyncValue<List<Channel>> next) {
    final channels = next.value;
    if (shown || !_notifier._hasLoaded || channels == null) return;
    if (identical(channels, _saved)) return;
    final lease = _lease(
      _notifier._memberSnapshotRelayBaseUrl ?? '',
      _notifier._memberSnapshotPubkey,
    );
    if (lease == null) return;
    _saveTimer?.cancel();
    _saveTimer = Timer(_channelListSaveDelay, () {
      _saveTimer = null;
      if (!_ref.mounted || shown) return;
      _saved = channels;
      unawaited(
        _ref.read(messageStoreProvider).writeSnapshots(lease, {
          channelListSnapshotKey: encodeChannelListSnapshot(channels),
        }),
      );
    });
  }

  void dispose() {
    _saveTimer?.cancel();
    _saveTimer = null;
  }
}

/// Waits until [ref]'s channel list is the relay's (or failed), past a saved
/// list drawn before it.
Future<void> waitForRelayChannelList(Ref ref) async {
  bool settled() {
    final state = ref.read(channelsProvider);
    return (state.hasValue &&
            !ref.read(channelsProvider.notifier).showsSavedList) ||
        (state.hasError && !state.isLoading) ||
        ref.read(channelListLoadErrorProvider) != null;
  }

  if (settled()) return;
  final done = Completer<void>();
  void check() {
    if (settled() && !done.isCompleted) done.complete();
  }

  final subscriptions = [
    ref.listen(channelsProvider, (_, _) => check()),
    ref.listen(channelListLoadErrorProvider, (_, _) => check()),
  ];
  try {
    await done.future;
  } finally {
    for (final subscription in subscriptions) {
      subscription.close();
    }
  }
}
