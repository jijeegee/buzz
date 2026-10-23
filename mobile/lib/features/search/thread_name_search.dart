import '../chats/chat_list.dart';

/// Most threads one relay name search returns.
const threadNameSearchLimit = 20;

/// Longest query the relay's `thread_search` accepts, in characters.
const threadNameSearchMaxChars = 100;

/// The relay's `thread_search` filter: threads in this account's rooms whose
/// name contains [query], closed and quiet ones included. Null when the
/// query is blank or too long to send.
Map<String, dynamic>? buildThreadSearchFilter(String query) {
  final trimmed = query.trim();
  if (trimmed.isEmpty || trimmed.runes.length > threadNameSearchMaxChars) {
    return null;
  }
  return {'thread_search': 1, 'q': trimmed, 'limit': threadNameSearchLimit};
}

/// Parses a `thread_search` response into Chats thread rows (no counts or
/// events). Anything but the version-1 envelope throws
/// [ChatListUnsupportedException]; malformed rows are skipped.
List<ChatListRow> parseThreadSearchResponse(Object? json) {
  if (json is! Map<String, dynamic> ||
      json['thread_search'] != 1 ||
      json['rows'] is! List) {
    throw const ChatListUnsupportedException();
  }
  return [for (final value in json['rows'] as List) ?_threadRow(value)];
}

ChatListRow? _threadRow(Object? json) {
  final row = ChatListRow.fromJson(json);
  return row != null && row.isThread ? row : null;
}

/// [chatRows] followed by the [searched] threads they do not already list,
/// so a thread in both keeps its Chats row.
List<ChatListRow> withSearchedThreads(
  List<ChatListRow> chatRows,
  List<ChatListRow> searched,
) {
  if (searched.isEmpty) return chatRows;
  final listed = {
    for (final row in chatRows)
      if (row.isThread) row.rootId ?? row.key,
  };
  return [
    ...chatRows,
    for (final row in searched)
      if (listed.add(row.rootId ?? row.key)) row,
  ];
}
