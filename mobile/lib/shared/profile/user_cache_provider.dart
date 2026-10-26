import 'dart:async';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../community/community_provider.dart';
import '../push/push_presentation_cache.dart';
import '../push/push_presentation_export_recovery.dart';
import '../relay/relay.dart';
import '../theme/theme_provider.dart';
import 'profile_label_storage.dart';
import 'user_profile.dart';
import 'profile_event_parser.dart';

/// Cache of user profiles, fetched in batches from the relay.
///
/// Starts from the profiles saved by earlier launches (name, avatar, handle),
/// so known people paint at once; each saved profile is read again from the
/// relay the first time it is asked for, in the background, and replaced if
/// it changed.
///
/// Lookups requested via [get] or [preload] are coalesced into a single
/// kind:0 batch query (NIP-01 `authors` filter) every 50ms. Larger requests
/// drain sequentially in pages bounded by the relay response limit.
class UserCacheNotifier extends Notifier<Map<String, UserProfile>> {
  // The relay clamps each history response to DEFAULT_MAX_PAGE_LIMIT (1000).
  static const _maximumProfilesPerQuery = 1000;

  final Set<String> _pending = {};
  final _pushExport = PushPresentationExportRecovery();
  final Map<String, ({int createdAt, String eventId})> _profileEventOrders = {};
  int _generation = 0;
  bool _flushInFlight = false;
  Future<void> _verificationQueue = Future.value();
  Timer? _batchTimer;
  Completer<bool>? _batchCompleter;

  /// Saved profiles not yet read again from the relay this launch.
  final Set<String> _stale = {};
  Timer? _saveTimer;

  @override
  Map<String, UserProfile> build() {
    final config = ref.watch(relayConfigProvider);
    _generation++;
    _profileEventOrders.clear();
    _stale.clear();
    final prefs = _prefs();
    ref.onDispose(() {
      _generation++;
      _pending.clear();
      _batchTimer?.cancel();
      _batchTimer = null;
      _batchCompleter?.complete(false);
      _batchCompleter = null;
      _saveTimer?.cancel();
      _saveTimer = null;
    });
    if (prefs == null) return {};
    // Saved shortly after the cache settles, not on every merge.
    listenSelf((previous, next) {
      if (identical(previous, next)) return;
      _saveTimer?.cancel();
      _saveTimer = Timer(const Duration(seconds: 2), () {
        unawaited(writeStoredProfiles(prefs, config.baseUrl, next));
      });
    });
    final stored = readStoredProfiles(prefs, config.baseUrl);
    _stale.addAll(stored.keys);
    return stored;
  }

  SharedPreferences? _prefs() {
    try {
      return ref.read(savedPrefsProvider);
    } catch (_) {
      // Not provided (tests): profiles are kept in memory only.
      return null;
    }
  }

  /// Request a profile for [pubkey]. Returns immediately from cache if
  /// available (a saved one is also read again in the background), otherwise
  /// schedules a batch fetch.
  UserProfile? get(String pubkey) {
    final key = pubkey.toLowerCase();
    final cached = state[key];
    if (cached != null) {
      if (_stale.contains(key)) _scheduleFetch(key);
      return cached;
    }
    _scheduleFetch(key);
    return null;
  }

  /// Stores a profile that was fetched or updated outside the batch loader.
  void put(UserProfile profile) {
    state = {...state, profile.pubkey.toLowerCase(): profile};
  }

  /// Preload profiles for a list of pubkeys (e.g. channel members).
  /// Returns whether the batch completed successfully.
  Future<bool> preload(List<String> pubkeys) {
    final normalized = pubkeys.map((pk) => pk.toLowerCase()).toSet();
    final alreadyPending = normalized.any(_pending.contains);
    final uncached = normalized
        .where((pk) => !state.containsKey(pk) && !_pending.contains(pk))
        .toList();
    // Saved profiles are shown already: read them again in the background,
    // never making the caller wait for them.
    final stale = normalized
        .where((pk) => _stale.contains(pk) && !_pending.contains(pk))
        .toList();
    if (stale.isNotEmpty) {
      _stale.removeAll(stale);
      _pending.addAll(stale);
      _scheduleBatch();
    }
    if (uncached.isEmpty && !alreadyPending) return Future.value(true);
    _pending.addAll(uncached);
    final completer = _batchCompleter ??= Completer<bool>();
    _scheduleBatch();
    return completer.future;
  }

  /// Force-refresh profiles for identity-sensitive gates.
  ///
  /// Unlike [preload], this fetches cached pubkeys too so stale human profiles
  /// cannot be trusted after a verified agent-owner profile was published.
  Future<bool> refresh(List<String> pubkeys) async {
    final normalized = pubkeys
        .map((pubkey) => pubkey.toLowerCase())
        .where((pubkey) => pubkey.isNotEmpty)
        .toSet()
        .toList();
    if (normalized.isEmpty) return true;
    final generation = _generation;
    try {
      final session = ref.read(relaySessionProvider.notifier);
      for (final batch in _profileQueryBatches(normalized)) {
        if (!_isCurrent(generation)) return false;
        final events = await session.fetchHistory(
          NostrFilters.profilesBatch(batch),
        );
        if (!_isCurrent(generation)) return false;
        if (!await _verifyAndMerge(events, generation)) return false;
      }
      return true;
    } catch (_) {
      return false;
    }
  }

  /// Applies a live kind:0 profile event to the cache.
  ///
  /// Surfaces that keep a participant-scoped profile subscription can use this
  /// to update names and avatars without discarding the rest of the cache.
  void cacheProfileEvent(NostrEvent event) {
    if (event.kind != 0) return;
    final updated = Map<String, UserProfile>.from(state);
    if (_cacheProfileEvent(event, updated)) state = updated;
  }

  void _scheduleFetch(String pubkey) {
    if (_pending.contains(pubkey)) return;
    if (state.containsKey(pubkey) && !_stale.contains(pubkey)) return;
    _stale.remove(pubkey);
    _pending.add(pubkey);
    _batchCompleter ??= Completer<bool>();
    _scheduleBatch();
  }

  Future<void> _flushPending() async {
    _batchTimer = null;
    if (_pending.isEmpty) return;

    final pubkeys = _pending.toList();
    _pending.clear();
    final completer = _batchCompleter;
    _batchCompleter = null;

    _flushInFlight = true;
    final generation = _generation;
    var succeeded = false;
    try {
      final communityID = ref.read(activeCommunityProvider).value?.id;
      final session = ref.read(relaySessionProvider.notifier);
      var remaining = pubkeys.length;
      for (final batch in _profileQueryBatches(pubkeys)) {
        if (!_isCurrent(generation)) return;
        final events = await session.fetchHistory(
          NostrFilters.profilesBatch(batch),
        );
        if (!_isCurrent(generation)) return;
        if (!await _verifyAndMerge(events, generation)) return;
        remaining -= batch.length;
        if (remaining == 0) {
          // All requested pages are ready. Native export does not gate readers.
          succeeded = true;
          completer?.complete(true);
        }
        if (communityID != null) {
          // Drain each page before fetching another, retaining at most one raw
          // response while later requests coalesce as pubkeys in _pending.
          await _pushExport.export(
            () => cacheKuzzPushProfileEvents(communityID, events),
          );
        }
      }
    } catch (_) {
      // Silently fail — non-gating callers will just show pubkeys.
    } finally {
      if (completer != null && !completer.isCompleted) {
        completer.complete(succeeded);
      }
      _flushInFlight = false;
      if (ref.mounted && _pending.isNotEmpty) _scheduleBatch();
    }
  }

  Iterable<List<String>> _profileQueryBatches(List<String> pubkeys) sync* {
    for (
      var start = 0;
      start < pubkeys.length;
      start += _maximumProfilesPerQuery
    ) {
      final end = start + _maximumProfilesPerQuery;
      yield pubkeys.sublist(start, end < pubkeys.length ? end : pubkeys.length);
    }
  }

  void _scheduleBatch() {
    if (_flushInFlight) return;
    _batchTimer ??= Timer(const Duration(milliseconds: 50), _flushPending);
  }

  Future<bool> _verifyAndMerge(List<NostrEvent> events, int generation) {
    // Both refresh and preload share one worker slot. Keep this queue across
    // community changes so an old worker cannot overlap a new community's job.
    final result = _verificationQueue.then((_) async {
      if (!_isCurrent(generation)) return false;
      // Match live-cache admission before parsing. In particular, a malformed
      // stale profile must not fail a refresh that previously ignored it.
      final pending = [
        for (final event in events)
          if (event.kind == 0 &&
              _isNewer(event.pubkey.toLowerCase(), event.createdAt, event.id))
            event,
      ];
      if (pending.isEmpty) return true;
      final profiles = await ref.read(profileEventBatchParserProvider)(pending);
      if (!_isCurrent(generation)) return false;
      _mergeParsedProfiles(profiles);
      return true;
    });
    // Callers receive the original error; only the sequencing tail recovers so
    // one rejected batch does not poison all future refreshes.
    _verificationQueue = result.then<void>(
      (_) {},
      onError: (Object error, StackTrace stack) {},
    );
    return result;
  }

  bool _isCurrent(int generation) => ref.mounted && generation == _generation;

  void _mergeParsedProfiles(List<ParsedProfileEvent> profiles) {
    // Read the current state after the isolate completes: live events and other
    // batches may have installed newer profiles while this batch was parsing.
    final updated = Map<String, UserProfile>.from(state);
    var changed = false;
    for (final parsed in profiles) {
      if (!_isNewer(parsed.profile.pubkey, parsed.createdAt, parsed.eventId)) {
        continue;
      }
      updated[parsed.profile.pubkey] = parsed.profile;
      _profileEventOrders[parsed.profile.pubkey] = (
        createdAt: parsed.createdAt,
        eventId: parsed.eventId,
      );
      changed = true;
    }
    if (changed) state = updated;
  }

  bool _isNewer(String pubkey, int createdAt, String eventId) {
    final current = _profileEventOrders[pubkey];
    return current == null ||
        createdAt > current.createdAt ||
        (createdAt == current.createdAt &&
            eventId.compareTo(current.eventId) < 0);
  }

  bool _cacheProfileEvent(NostrEvent event, Map<String, UserProfile> profiles) {
    if (event.kind != 0) return false;
    final pubkey = event.pubkey.toLowerCase();
    if (!_isNewer(pubkey, event.createdAt, event.id)) return false;
    profiles[pubkey] = parseProfileEvent(event).profile;
    _profileEventOrders[pubkey] = (
      createdAt: event.createdAt,
      eventId: event.id,
    );
    return true;
  }
}

/// Profile history parser, isolated from provider state so batch completion can
/// be delayed independently of relay delivery.
final profileEventBatchParserProvider =
    Provider<Future<List<ParsedProfileEvent>> Function(List<NostrEvent>)>(
      (ref) => parseProfileEventBatch,
    );

final userCacheProvider =
    NotifierProvider<UserCacheNotifier, Map<String, UserProfile>>(
      UserCacheNotifier.new,
    );
