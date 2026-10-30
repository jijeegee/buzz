import 'dart:convert';

import 'user_profile.dart';

/// Saved profiles live in the account's store file under this prefix plus
/// the lower-case pubkey (docs/message-sync.md#launch-order).
const profileSnapshotPrefix = 'profile:';

/// The saved profiles kept per account and server, most recently saved first.
const profileSnapshotLimit = 500;

/// The account's own saved profile.
const selfProfileSnapshotKey = 'self_profile';

/// A saved profile with the kind:0 event it came from, when known.
typedef SavedProfile = ({UserProfile profile, int createdAt, String eventId});

String encodeProfileSnapshot(
  UserProfile profile, {
  int createdAt = 0,
  String eventId = '',
}) => jsonEncode({
  'pubkey': profile.pubkey,
  'name': profile.displayName,
  'picture': profile.avatarUrl,
  'about': profile.about,
  'nip05': profile.nip05Handle,
  'owner': profile.ownerPubkey,
  'created_at': createdAt,
  'event_id': eventId,
});

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
    );
  } catch (_) {
    return null;
  }
}
