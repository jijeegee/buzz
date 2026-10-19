/// Builds the kind:9002 tags used to update channel name, description, and
/// the per-room goals switch and visibility (`open`/`private`). Any one
/// field may be sent alone.
List<List<String>> buildUpdateChannelTags({
  required String channelId,
  String? name,
  String? description,
  bool? goalsEnabled,
  String? visibility,
}) {
  if (name == null &&
      description == null &&
      goalsEnabled == null &&
      visibility == null) {
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
  ];
}
