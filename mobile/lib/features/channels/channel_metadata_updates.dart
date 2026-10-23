/// Builds the kind:9002 tags used to update channel name, description, and
/// the per-room goals switch, visibility (`open`/`private`), the group
/// photo ([picture]; an empty string removes it) and whether new members see
/// earlier messages ([historyAll]: `all` or `since_join`). Any one field may
/// be sent alone.
List<List<String>> buildUpdateChannelTags({
  required String channelId,
  String? name,
  String? description,
  bool? goalsEnabled,
  String? visibility,
  String? picture,
  bool? historyAll,
}) {
  if (name == null &&
      description == null &&
      goalsEnabled == null &&
      visibility == null &&
      picture == null &&
      historyAll == null) {
    throw ArgumentError('At least one channel field must be provided');
  }
  final canonicalName = name?.trim().replaceFirst(RegExp(r'^#+'), '').trim();
  if (canonicalName != null && canonicalName.isEmpty) {
    throw ArgumentError.value(name, 'name', 'must not be empty');
  }
  return [
    ['h', channelId],
    if (canonicalName != null) ['name', canonicalName],
    if (description != null) ['about', description.trim()],
    if (goalsEnabled != null) ['goals', goalsEnabled ? 'on' : 'off'],
    if (visibility != null) ['visibility', visibility],
    if (picture != null) ['picture', picture.trim()],
    if (historyAll != null) ['history', historyAll ? 'all' : 'since_join'],
  ];
}
