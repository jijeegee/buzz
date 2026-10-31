import 'dart:convert';

import '../relay/nostr_models.dart';
import 'user_profile.dart';

/// Saved profiles live in the account's store file under this prefix plus
/// the lower-case pubkey (docs/message-sync.md#launch-order).
const profileSnapshotPrefix = 'profile:';

/// The saved profiles kept per account and server, most recently saved first.
const profileSnapshotLimit = 500;

/// Largest signed event (UTF-8 JSON bytes) saved with a profile; a bigger
/// one (say an inline picture) is left out to keep the store file small.
const profileSnapshotEventMaxBytes = 16 * 1024;

/// The account's own saved profile.
const selfProfileSnapshotKey = 'self_profile';

/// A saved profile with the kind:0 event it came from, when known, and when
/// (this device's clock, unix seconds) the relay last confirmed it; 0 when
/// unknown. [event] is that signed kind:0 event, kept so an unchanged
/// profile can be exported again (push presentation cache).
typedef SavedProfile = ({
  UserProfile profile,
  int createdAt,
  String eventId,
  int verifiedAt,
  NostrEvent? event,
});

String encodeProfileSnapshot(
  UserProfile profile, {
  int createdAt = 0,
  String eventId = '',
  int verifiedAt = 0,
  NostrEvent? event,
}) => jsonEncode({
  'pubkey': profile.pubkey,
  'name': profile.displayName,
  'picture': profile.avatarUrl,
  'about': profile.about,
  'nip05': profile.nip05Handle,
  'owner': profile.ownerPubkey,
  'created_at': createdAt,
  'event_id': eventId,
  'verified_at': verifiedAt,
  'event': ?_boundedEvent(event),
});

Map<String, dynamic>? _boundedEvent(NostrEvent? event) {
  if (event == null) return null;
  final json = event.toJson();
  return utf8.encode(jsonEncode(json)).length <= profileSnapshotEventMaxBytes
      ? json
      : null;
}

/// The saved profile in [raw], or `null` when it is unreadable.
SavedProfile? decodeProfileSnapshot(String? raw) {
  if (raw == null) return null;
  try {
    final json = jsonDecode(raw);
    if (json is! Map<String, dynamic>) return null;
    final pubkey = json['pubkey'];
    if (pubkey is! String || pubkey.isEmpty) return null;
    String? text(String key) =>
        json[key] is String ? json[key] as String : null;
    return (
      profile: UserProfile(
        pubkey: pubkey.toLowerCase(),
        displayName: text('name'),
        avatarUrl: text('picture'),
        about: text('about'),
        nip05Handle: text('nip05'),
        ownerPubkey: text('owner'),
      ),
      createdAt: json['created_at'] is int ? json['created_at'] as int : 0,
      eventId: text('event_id') ?? '',
      verifiedAt: json['verified_at'] is int ? json['verified_at'] as int : 0,
      event: _savedEvent(json['event'], pubkey, text('event_id')),
    );
  } catch (_) {
    return null;
  }
}

/// The signed kind:0 event saved with a profile, when it is that profile's
/// event ([eventId]).
NostrEvent? _savedEvent(Object? raw, String pubkey, String? eventId) {
  if (raw is! Map<String, dynamic> || eventId == null) return null;
  try {
    final event = NostrEvent.fromJson(raw);
    return event.kind == 0 &&
            event.sig != null &&
            event.id == eventId &&
            event.pubkey.toLowerCase() == pubkey.toLowerCase()
        ? event
        : null;
  } catch (_) {
    return null;
  }
}
