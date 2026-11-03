import 'push_subscription.dart';

/// Which fetched events a push policy covers: a Dart port of the iPhone
/// extension's `PushLeaseMatcher` (KuzzPushKit `PushLease.swift`), so the
/// Android background handler keeps exactly what the iPhone keeps. Both
/// are checked against the `matcher` cases of test-fixtures/push-inbox.json.
///
/// Events are raw NIP-01 JSON maps, as the relay's `/query` returns them.

/// Whether [event] passes [filter]: its kind is listed, and each listed
/// author, `#p`, `#h` and `#e` set has a match (case-insensitive values).
bool pushFilterMatches(KuzzPushFilter filter, Map<String, dynamic> event) {
  final kind = event['kind'];
  final pubkey = event['pubkey'];
  if (kind is! int || !filter.kinds.contains(kind)) return false;
  final authors = filter.authors;
  if (authors != null &&
      (pubkey is! String || !authors.contains(pubkey.toLowerCase()))) {
    return false;
  }
  for (final (name, values) in [
    ('p', filter.pTags),
    ('h', filter.hTags),
    ('e', filter.eTags),
  ]) {
    if (values != null && !_hasAnyTag(event, name, values)) return false;
  }
  return true;
}

/// Whether [policy] pushes [event]: its filter matches, no ignore filter
/// does, and the event has no more `p` tags than `suppress.p_tags_max`.
bool pushPolicyMatches(
  KuzzPushSubscription policy,
  Map<String, dynamic> event,
) {
  if (!pushFilterMatches(policy.filter, event)) return false;
  if (policy.ignore.any((filter) => pushFilterMatches(filter, event))) {
    return false;
  }
  final maximum = policy.suppress?.pTagsMax;
  return maximum == null || _tagCount(event, 'p') <= maximum;
}

/// Whether any of [policies] pushes [event].
bool pushPoliciesMatch(
  Iterable<KuzzPushSubscription> policies,
  Map<String, dynamic> event,
) => policies.any((policy) => pushPolicyMatches(policy, event));

/// Whether [event] is kept only as a sender profile (kind:0), never as a
/// pushed message. Group metadata (39000, 39002) is never kept.
bool pushKeepsAsProfile(Map<String, dynamic> event) => event['kind'] == 0;

Iterable<List<Object?>> _tags(Map<String, dynamic> event) sync* {
  final tags = event['tags'];
  if (tags is! List) return;
  for (final tag in tags) {
    if (tag is List && tag.length >= 2) yield tag;
  }
}

int _tagCount(Map<String, dynamic> event, String name) =>
    _tags(event).where((tag) => tag[0] == name).length;

bool _hasAnyTag(Map<String, dynamic> event, String name, List<String> values) {
  final expected = {for (final value in values) value.toLowerCase()};
  return _tags(event).any((tag) {
    final value = tag[1];
    return tag[0] == name &&
        value is String &&
        expected.contains(value.toLowerCase());
  });
}
