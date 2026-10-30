part of 'channels_provider.dart';

/// The error of the relay's channel list while the saved list (or, right
/// after an account switch, none) stays shown; `null` when there is none.
/// The pages keep the list and offer a small retry; the notifier also asks
/// again by itself (docs/message-sync.md#launch-snapshots).
final channelListLoadErrorProvider =
    NotifierProvider<ChannelListLoadError, Object?>(ChannelListLoadError.new);

class ChannelListLoadError extends Notifier<Object?> {
  @override
  Object? build() => null;

  void _set(Object? error) {
    if (!identical(state, error)) state = error;
  }
}

/// How long the relay's list waits before it is asked for again after a
/// failure; past the last, a reconnect or a return to the app asks again.
const _relayListRetryDelays = [
  Duration(seconds: 2),
  Duration(seconds: 5),
  Duration(seconds: 15),
];

/// Recovery of the channel list drawn from saved data: retries a failed
/// relay list with backoff, and asks once again for the profiles DM names
/// were drawn from.
class _ChannelListRecovery {
  _ChannelListRecovery(this._notifier);

  final ChannelsNotifier _notifier;

  int _attempt = 0;
  Timer? _timer;
  int _scope = 0;
  final Set<String> _dmProfilesRechecked = {};

  Ref get _ref => _notifier._lifecycleRef;

  void _report(Object? error) {
    final scope = _scope;
    // Never written while a provider builds.
    Future.microtask(() {
      if (!_ref.mounted || scope != _scope) return;
      _ref.read(channelListLoadErrorProvider.notifier)._set(error);
    });
  }

  void _stopRetrying() {
    _attempt = 0;
    _timer?.cancel();
    _timer = null;
  }

  /// A new relay or account: nothing of the previous one is retried or
  /// reported.
  void resetScope() {
    _stopRetrying();
    _dmProfilesRechecked.clear();
    _scope++;
    final scope = _scope;
    Future.microtask(() {
      if (!_ref.mounted || scope != _scope) return;
      _ref.read(channelListLoadErrorProvider.notifier)._set(null);
    });
  }

  /// The relay's list failed while the saved one is shown: it stays, the
  /// error is reported and the list is asked for again later.
  void failed(Object error) {
    _report(error);
    if (_timer != null || _attempt >= _relayListRetryDelays.length) return;
    _timer = Timer(_relayListRetryDelays[_attempt++], () {
      _timer = null;
      if (_ref.mounted) unawaited(_notifier.refresh());
    });
  }

  /// The relay's list arrived.
  void loaded() {
    _stopRetrying();
    _report(null);
  }

  /// The page's retry: asks now and starts the backoff over.
  Future<void> retryNow() {
    _stopRetrying();
    return _notifier.refresh();
  }

  /// Asks once more, in one batch, for the people whose DM names were drawn
  /// from saved profiles, and renames the DMs whose names changed.
  void recheckDmNames(Set<String> pubkeys) {
    final ask = pubkeys.difference(_dmProfilesRechecked);
    if (ask.isEmpty) return;
    _dmProfilesRechecked.addAll(ask);
    final scope = _scope;
    // After the list this request returns is shown.
    Future.microtask(() async {
      if (!_ref.mounted) return;
      final asked = await _ref
          .read(userCacheProvider.notifier)
          .refresh(ask.toList());
      if (!asked || !_ref.mounted || scope != _scope) return;
      _renameDms(_ref.read(userCacheProvider));
    });
  }

  void _renameDms(Map<String, UserProfile> profiles) {
    final current = _ref.read(channelsProvider).value;
    if (current == null || _notifier.showsSavedList) return;
    final me = _notifier._memberSnapshotPubkey;
    var changed = false;
    final next = [
      for (final channel in current)
        if (!channel.isDm)
          channel
        else
          () {
            final labels = [
              for (var i = 0; i < channel.participantPubkeys.length; i++)
                () {
                  final pubkey = channel.participantPubkeys[i].toLowerCase();
                  final shown = i < channel.participants.length
                      ? channel.participants[i]
                      : shortPubkey(pubkey);
                  final profile = profiles[pubkey];
                  return pubkey == me || profile == null
                      ? shown
                      : _dmLabel(
                          pubkey,
                          profile.displayName,
                          profile.nip05Handle,
                        );
                }(),
            ];
            if (listEquals(labels, channel.participants)) return channel;
            changed = true;
            return channel.copyWith(participants: labels);
          }(),
    ];
    if (changed) _notifier._showList(next);
  }

  void dispose() => _stopRetrying();
}
