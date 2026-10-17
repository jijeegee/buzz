import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:shared_preferences/shared_preferences.dart';

import '../../shared/community/community_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';
import '../channels/channel_stars/channel_stars_manager.dart'
    show ChannelStarsCrypto;

/// The synced lane desktop's People screen uses (`PEOPLE_FAVORITES_LANE` in
/// `desktop/src/features/sidebar/lib/syncedIdSetLane.ts`): an encrypted-to-self
/// kind 30078 record, one last-writer-wins register per person.
const peopleFavoritesDTag = 'people-favorites';

/// Desktop's cap; adding past it is refused.
const maxPeopleFavorites = 200;

/// Removals kept as `false` registers so a removal reaches devices that still
/// hold the person (desktop's `MAX_ID_SET_TOMBSTONES`).
const maxPeopleFavoriteTombstones = 50;

const _maxCiphertextBytes = 65535;
final _devPattern = RegExp(r'^[0-9a-f]{16}$');

/// One register `[v, dev, member]`: a millisecond version, the writing
/// install's id, and whether the person is a favorite.
@immutable
class FavoriteRegister {
  const FavoriteRegister(this.version, this.device, this.member);

  final int version;
  final String device;
  final bool member;

  List<Object> toJson() => [version, device, member];

  /// Desktop's `compareRegs`: version, then device bytes, then a removal over
  /// an add. Values are booleans, so nothing further can differ.
  int compareTo(FavoriteRegister other) {
    if (version != other.version) return version.compareTo(other.version);
    if (device != other.device) return device.compareTo(other.device);
    if (member != other.member) return member ? -1 : 1;
    return 0;
  }

  @override
  bool operator ==(Object other) =>
      other is FavoriteRegister &&
      other.version == version &&
      other.device == device &&
      other.member == member;

  @override
  int get hashCode => Object.hash(version, device, member);
}

typedef FavoriteTree = Map<String, FavoriteRegister>;

/// Merges [b] into [a]: per person, the register that sorts highest wins, so
/// any two devices that saw the same registers hold the same tree.
FavoriteTree mergeFavoriteTrees(FavoriteTree a, FavoriteTree b) {
  final out = Map<String, FavoriteRegister>.of(a);
  b.forEach((id, reg) {
    final current = out[id];
    if (current == null || reg.compareTo(current) > 0) out[id] = reg;
  });
  return out;
}

/// The favorite people in [tree], sorted.
List<String> favoriteIdsOf(FavoriteTree tree) => [
  for (final entry in tree.entries)
    if (entry.value.member) entry.key,
]..sort();

/// The wire document, exactly as desktop's `encodeDoc` writes it.
Map<String, Object?> encodeFavoritesDoc(FavoriteTree tree) => {
  'version': 1,
  'ids': favoriteIdsOf(tree),
  'meta': {
    'v': 1,
    'i': {for (final entry in tree.entries) entry.key: entry.value.toJson()},
  },
};

/// The registers of a wire document, or `null` when it is not one this
/// version understands. Malformed registers are dropped, valid siblings kept.
FavoriteTree? decodeFavoritesDoc(Object? json) {
  if (json is! Map<String, Object?> || json['version'] != 1) return null;
  final meta = json['meta'];
  if (meta is! Map<String, Object?> || meta['v'] != 1) return null;
  final node = meta['i'];
  if (node == null) return {};
  if (node is! Map<String, Object?>) return null;
  final tree = <String, FavoriteRegister>{};
  node.forEach((id, value) {
    if (id == '__proto__' || value is! List || value.length != 3) return;
    final [version, device, member] = value;
    if (version is! int || version < 0) return;
    if (device is! String || !_devPattern.hasMatch(device)) return;
    if (member is! bool) return;
    tree[id] = FavoriteRegister(version, device, member);
  });
  return tree;
}

/// Key-sorted JSON: the digest desktop compares documents by.
String canonicalJson(Object? value) {
  if (value is List) return '[${value.map(canonicalJson).join(',')}]';
  if (value is Map) {
    final keys = [for (final key in value.keys) key as String]
      ..sort(_compareUtf8);
    return '{${keys.map((k) => '${jsonEncode(k)}:${canonicalJson(value[k])}').join(',')}}';
  }
  return jsonEncode(value);
}

int _compareUtf8(String a, String b) {
  final left = utf8.encode(a);
  final right = utf8.encode(b);
  for (var i = 0; i < min(left.length, right.length); i++) {
    if (left[i] != right[i]) return left[i] - right[i];
  }
  return left.length - right.length;
}

/// Drops the oldest removals beyond [maxPeopleFavoriteTombstones].
FavoriteTree _pruneTombstones(FavoriteTree tree) {
  final tombstones = [
    for (final entry in tree.entries)
      if (!entry.value.member) entry,
  ]..sort((a, b) => b.value.version.compareTo(a.value.version));
  if (tombstones.length <= maxPeopleFavoriteTombstones) return tree;
  final out = Map<String, FavoriteRegister>.of(tree);
  for (final entry in tombstones.skip(maxPeopleFavoriteTombstones)) {
    out.remove(entry.key);
  }
  return out;
}

enum _HeadStatus { unknown, empty, decoded, unreadable }

/// Keeps one account's favorite people in step with its other devices.
///
/// Local edits persist at once and publish after a short debounce. Before
/// publishing it re-reads the relay head and merges it, and it never
/// publishes over a head it could not read (one written by a newer version)
/// or while the read itself failed. Failed publishes retry with bounded
/// backoff; edits and foreground resumes start a fresh round.
class PeopleFavoritesManager {
  PeopleFavoritesManager({
    required this.pubkey,
    required String origin,
    required SharedPreferences prefs,
    required ChannelStarsCrypto? crypto,
    required RelaySessionNotifier? relaySession,
    required SignedEventRelay? signedEventRelay,
    required bool remoteEnabled,
    required VoidCallback onChanged,
    @visibleForTesting Duration debounce = const Duration(seconds: 2),
    @visibleForTesting int Function()? nowMs,
  }) : _prefs = prefs,
       _storageKey = 'kuzz-people-favorites.v1:$pubkey:$origin',
       _crypto = crypto,
       _relaySession = relaySession,
       _signedEventRelay = signedEventRelay,
       _remoteEnabled =
           remoteEnabled &&
           crypto != null &&
           relaySession != null &&
           signedEventRelay != null,
       _onChanged = onChanged,
       _debounce = debounce,
       _nowMs = nowMs ?? (() => DateTime.now().millisecondsSinceEpoch) {
    _tree = _readLocal();
  }

  final String pubkey;
  final SharedPreferences _prefs;
  final String _storageKey;
  final ChannelStarsCrypto? _crypto;
  final RelaySessionNotifier? _relaySession;
  final SignedEventRelay? _signedEventRelay;
  final bool _remoteEnabled;
  final VoidCallback _onChanged;
  final Duration _debounce;
  final int Function() _nowMs;

  static const _clockKey = 'kuzz-people-favorites-clock.v1';
  static const _backoff = [
    Duration(seconds: 5),
    Duration(seconds: 10),
    Duration(seconds: 30),
    Duration(seconds: 60),
  ];

  FavoriteTree _tree = {};
  _HeadStatus _headStatus = _HeadStatus.unknown;
  String? _headDigest;
  String _headId = '';
  int _headCreatedAt = 0;
  bool _headSeen = false;
  Timer? _timer;
  int _failures = 0;
  Future<void>? _inFlight;
  bool _again = false;
  void Function()? _unsubscribe;
  bool _disposed = false;

  Set<String> get ids => favoriteIdsOf(_tree).toSet();

  @visibleForTesting
  FavoriteTree get tree => Map.unmodifiable(_tree);

  Future<void> initialize() async {
    if (_disposed || !_remoteEnabled) return;
    await _read();
    if (_disposed) return;
    try {
      _unsubscribe = await _relaySession!.subscribe(
        _filter(0),
        (event) => unawaited(_ingest(event).then((_) => _reconcile())),
      );
      if (_disposed) _unsubscribe?.call();
    } catch (error) {
      debugPrint('[PeopleFavorites] live subscription failed: $error');
    }
    unawaited(_reconcile());
  }

  /// Re-reads the head once, e.g. on foreground resume.
  void refreshFromRelay() {
    if (_disposed || !_remoteEnabled) return;
    _failures = 0;
    unawaited(_read().then((_) => _reconcile()));
  }

  /// False when [maxPeopleFavorites] people are already favorites.
  bool add(String id) {
    final key = id.toLowerCase();
    if (_tree[key]?.member == true) return true;
    if (ids.length >= maxPeopleFavorites) return false;
    _write(key, true);
    return true;
  }

  void remove(String id) {
    final key = id.toLowerCase();
    // Desktop writes no removal for a person it never held.
    if (_tree[key] == null || _tree[key]!.member == false) return;
    _write(key, false);
  }

  void dispose() {
    _disposed = true;
    _timer?.cancel();
    _unsubscribe?.call();
  }

  void _write(String id, bool member) {
    if (_disposed) return;
    final version = _nextVersion();
    _tree = _pruneTombstones({
      ..._tree,
      id: FavoriteRegister(version.$1, version.$2, member),
    });
    unawaited(_persist());
    _onChanged();
    if (!_remoteEnabled) return;
    _failures = 0;
    _schedule(_debounce);
  }

  /// `v = max(now, lastIssued + 1, maxSeen + 1)` with a per-install device id.
  (int, String) _nextVersion() {
    var dev = '';
    var last = 0;
    try {
      final saved = jsonDecode(_prefs.getString(_clockKey) ?? 'null');
      if (saved is Map<String, Object?> &&
          saved['dev'] is String &&
          _devPattern.hasMatch(saved['dev']! as String) &&
          saved['last'] is int) {
        dev = saved['dev']! as String;
        last = saved['last']! as int;
      }
    } on FormatException {
      // A fresh clock still orders after every version seen below.
    }
    if (dev.isEmpty) {
      final random = Random.secure();
      dev = List.generate(
        8,
        (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
      ).join();
    }
    final maxSeen = _tree.values.fold<int>(0, (m, r) => max(m, r.version));
    final version = max(_nowMs(), max(last + 1, maxSeen + 1));
    unawaited(
      _prefs.setString(_clockKey, jsonEncode({'dev': dev, 'last': version})),
    );
    return (version, dev);
  }

  FavoriteTree _readLocal() {
    try {
      final raw = _prefs.getString(_storageKey);
      if (raw == null) return {};
      return decodeFavoritesDoc(jsonDecode(raw)) ?? {};
    } on FormatException {
      return {};
    }
  }

  Future<void> _persist() async {
    try {
      await _prefs.setString(
        _storageKey,
        canonicalJson(encodeFavoritesDoc(_tree)),
      );
    } catch (error) {
      debugPrint('[PeopleFavorites] persist failed: $error');
    }
  }

  NostrFilter _filter(int limit) => NostrFilter(
    kinds: const [EventKind.readState],
    authors: [pubkey],
    tags: const {
      '#d': [peopleFavoritesDTag],
    },
    limit: limit,
  );

  /// The relay's replaceable winner: later `created_at`, then lower id.
  bool _beatsHead(NostrEvent event) =>
      event.createdAt > _headCreatedAt ||
      (event.createdAt == _headCreatedAt && event.id.compareTo(_headId) < 0);

  /// One head read; false when it failed.
  Future<bool> _read() async {
    final List<NostrEvent> events;
    try {
      events = await _relaySession!.fetchHistory(_filter(1));
    } catch (error) {
      debugPrint('[PeopleFavorites] read failed: $error');
      return false;
    }
    if (_disposed) return false;
    final own = events.where(_isOwnLaneEvent).toList();
    if (own.isEmpty) {
      // Absence lets a first copy publish only before any head was seen.
      _headStatus = _headSeen ? _HeadStatus.unknown : _HeadStatus.empty;
      _headDigest = null;
      return true;
    }
    for (final event in own) {
      await _ingest(event);
    }
    return true;
  }

  bool _isOwnLaneEvent(NostrEvent event) =>
      event.pubkey == pubkey &&
      event.kind == EventKind.readState &&
      event.getTagValue('d') == peopleFavoritesDTag;

  Future<void> _ingest(NostrEvent event) async {
    if (_disposed || !_isOwnLaneEvent(event)) return;
    _headSeen = true;
    final isHead = _beatsHead(event) || event.id == _headId;
    Object? json;
    FavoriteTree? decoded;
    try {
      json = jsonDecode(_crypto!.decrypt(event.content));
      decoded = decodeFavoritesDoc(json);
    } catch (_) {
      decoded = null;
    }
    if (decoded != null) {
      final merged = mergeFavoriteTrees(_tree, decoded);
      if (canonicalJson(encodeFavoritesDoc(merged)) !=
          canonicalJson(encodeFavoritesDoc(_tree))) {
        _tree = merged;
        await _persist();
        if (!_disposed) _onChanged();
      }
    }
    if (!isHead) return;
    _headId = event.id;
    _headCreatedAt = event.createdAt;
    _headStatus = decoded == null
        ? _HeadStatus.unreadable
        : _HeadStatus.decoded;
    _headDigest = decoded == null ? null : canonicalJson(json);
  }

  bool get _settled {
    final doc = canonicalJson(encodeFavoritesDoc(_tree));
    return switch (_headStatus) {
      _HeadStatus.empty => _tree.isEmpty,
      _HeadStatus.decoded => _headDigest == doc,
      // Hold: never write over a head we could not read.
      _HeadStatus.unknown || _HeadStatus.unreadable => true,
    };
  }

  void _schedule(Duration delay) {
    if (_disposed) return;
    _timer?.cancel();
    _timer = Timer(delay, () {
      _timer = null;
      unawaited(_publish());
    });
  }

  Future<void> _reconcile() async {
    if (_disposed || !_remoteEnabled || _timer != null) return;
    if (_settled) return;
    await _publish();
  }

  Future<void> _publish() {
    final running = _inFlight;
    if (running != null) {
      _again = true;
      return running;
    }
    final attempt = _attemptPublish();
    _inFlight = attempt;
    return attempt.whenComplete(() {
      _inFlight = null;
      if (_again && !_disposed) {
        _again = false;
        unawaited(_publish());
      }
    });
  }

  Future<void> _attemptPublish() async {
    if (_disposed || !_remoteEnabled) return;
    // Preflight: merge the current head first.
    if (!await _read()) return _retryLater();
    if (_disposed || _settled) return;
    final status = _headStatus;
    if (status != _HeadStatus.empty && status != _HeadStatus.decoded) return;
    final doc = encodeFavoritesDoc(_tree);
    if ((doc['ids']! as List).length > maxPeopleFavorites) return;
    final content = _crypto!.encrypt(jsonEncode(doc));
    if (utf8.encode(content).length > _maxCiphertextBytes) return;
    final createdAt = max(_nowMs() ~/ 1000, _headCreatedAt + 1);
    String? signedId;
    try {
      await _signedEventRelay!.submit(
        kind: EventKind.readState,
        content: content,
        tags: const [
          ['d', peopleFavoritesDTag],
          ['t', peopleFavoritesDTag],
        ],
        createdAt: createdAt,
        onSigned: (event) => signedId = event.id,
      );
    } catch (error) {
      if (!'$error'.contains('duplicate:')) {
        debugPrint('[PeopleFavorites] publish failed: $error');
        return _retryLater();
      }
    }
    _failures = 0;
    _headSeen = true;
    final id = signedId;
    if (id != null &&
        (createdAt > _headCreatedAt ||
            (createdAt == _headCreatedAt && id.compareTo(_headId) < 0))) {
      _headId = id;
      _headCreatedAt = createdAt;
      _headStatus = _HeadStatus.decoded;
      _headDigest = canonicalJson(doc);
    }
  }

  void _retryLater() {
    if (_disposed || _failures >= _backoff.length) return;
    final delay = _backoff[_failures];
    _failures++;
    _schedule(delay);
  }
}

/// My favorite people, shared with desktop's People screen and my other
/// devices. On a community without a signing key the list stays on this
/// device.
class PeopleFavoritesNotifier extends Notifier<Set<String>> {
  PeopleFavoritesManager? _manager;

  @override
  Set<String> build() {
    final config = ref.watch(relayConfigProvider);
    final status = ref.watch(relaySessionProvider.select((s) => s.status));
    ref.watch(activeCommunityProvider);
    final trimmed = config.tokenAuth ? null : config.nsec?.trim();
    final nsec = trimmed == null || trimmed.isEmpty ? null : trimmed;
    final pubkey = nsec == null
        ? (config.tokenAuth ? config.principalId : null)
        : _pubkeyFromNsec(nsec);
    if (pubkey == null || pubkey.isEmpty) return const <String>{};

    ChannelStarsCrypto? crypto;
    if (nsec != null) {
      try {
        crypto = ChannelStarsCrypto(nsec, pubkey);
      } catch (_) {
        crypto = null;
      }
    }
    final session = ref.read(relaySessionProvider.notifier);
    late final PeopleFavoritesManager manager;
    manager = PeopleFavoritesManager(
      pubkey: pubkey,
      origin: config.baseUrl,
      prefs: ref.read(savedPrefsProvider),
      crypto: crypto,
      relaySession: session,
      signedEventRelay: nsec == null
          ? null
          : SignedEventRelay(session: session, nsec: nsec),
      remoteEnabled: status == SessionStatus.connected,
      onChanged: () {
        if (_manager == manager) state = manager.ids;
      },
    );
    _manager = manager;
    ref.listen(appLifecycleProvider, (_, next) {
      if (next == AppLifecycleState.resumed) manager.refreshFromRelay();
    });
    ref.onDispose(() {
      manager.dispose();
      if (_manager == manager) _manager = null;
    });
    Future.microtask(manager.initialize);
    return manager.ids;
  }

  bool isFavorite(String pubkey) => state.contains(pubkey.toLowerCase());

  /// False when the list is full.
  bool add(String pubkey) => _manager?.add(pubkey) ?? false;

  void remove(String pubkey) => _manager?.remove(pubkey);
}

final peopleFavoritesProvider =
    NotifierProvider<PeopleFavoritesNotifier, Set<String>>(
      PeopleFavoritesNotifier.new,
    );

String? _pubkeyFromNsec(String nsec) {
  try {
    final privkeyHex = nostr.Nip19.decode(payload: nsec).data;
    if (privkeyHex.isEmpty) return null;
    return nostr.Keys(privkeyHex).public;
  } catch (_) {
    return null;
  }
}

/// The full-list message desktop shows.
const peopleFavoritesFullMessage = 'You can favorite up to 200 people.';
