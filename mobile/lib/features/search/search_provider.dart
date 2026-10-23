import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../channels/channel.dart';
import '../channels/channel_management_provider.dart';
import '../channels/channels_provider.dart';
import '../chats/chat_list.dart';
import 'thread_name_search.dart';

@immutable
class SearchHit {
  final String eventId;
  final String content;
  final int kind;
  final String pubkey;
  final String? channelId;
  final String? channelName;
  final int createdAt;
  final double score;
  final List<List<String>> tags;

  const SearchHit({
    required this.eventId,
    required this.content,
    required this.kind,
    required this.pubkey,
    this.channelId,
    this.channelName,
    required this.createdAt,
    required this.score,
    this.tags = const [],
  });

  factory SearchHit.fromJson(Map<String, dynamic> json) => SearchHit(
    eventId: json['event_id'] as String,
    content: json['content'] as String? ?? '',
    kind: json['kind'] as int? ?? 9,
    pubkey: json['pubkey'] as String,
    channelId: json['channel_id'] as String?,
    channelName: json['channel_name'] as String?,
    createdAt: json['created_at'] as int? ?? 0,
    score: (json['score'] as num?)?.toDouble() ?? 0.0,
    tags:
        (json['tags'] as List<dynamic>?)
            ?.map((t) => (t as List<dynamic>).map((e) => e as String).toList())
            .toList() ??
        const [],
  );
}

@immutable
class SearchState {
  final String query;
  final List<SearchHit> messageResults;
  final List<DirectoryUser> userResults;

  /// Threads the relay found by name for [query], closed and quiet ones
  /// included (the Chats list leaves those out).
  final List<ChatListRow> threadResults;
  final bool isLoading;
  final String? error;

  const SearchState({
    this.query = '',
    this.messageResults = const [],
    this.userResults = const [],
    this.threadResults = const [],
    this.isLoading = false,
    this.error,
  });

  const SearchState.initial()
    : query = '',
      messageResults = const [],
      userResults = const [],
      threadResults = const [],
      isLoading = false,
      error = null;

  SearchState copyWith({
    String? query,
    List<SearchHit>? messageResults,
    List<DirectoryUser>? userResults,
    List<ChatListRow>? threadResults,
    bool? isLoading,
    String? error,
  }) => SearchState(
    query: query ?? this.query,
    messageResults: messageResults ?? this.messageResults,
    userResults: userResults ?? this.userResults,
    threadResults: threadResults ?? this.threadResults,
    isLoading: isLoading ?? this.isLoading,
    error: error ?? this.error,
  );
}

class SearchNotifier extends Notifier<SearchState> {
  Timer? _debounce;

  @override
  SearchState build() {
    // Watch relayConfig so search state resets on community switch.
    ref.watch(relayConfigProvider);
    ref.onDispose(() {
      _debounce?.cancel();
    });
    return const SearchState.initial();
  }

  void search(String query) {
    _debounce?.cancel();

    final trimmed = query.trim();
    if (trimmed.isEmpty) {
      state = const SearchState.initial();
      return;
    }

    state = SearchState(query: trimmed, isLoading: true);

    _debounce = Timer(const Duration(milliseconds: 300), () {
      _executeSearch(trimmed);
    });
  }

  Future<void> _executeSearch(String query) async {
    // If the query changed while debouncing, bail out.
    if (state.query != query) return;

    // Chats by name come from searchChatResultsProvider: this device's
    // Chats rows plus the relay's thread-name search.
    _searchMessages(query);
    _searchUsers(query);
    _searchThreadNames(query);
  }

  Future<void> _searchThreadNames(String query) async {
    final filter = buildThreadSearchFilter(query);
    if (filter == null) return;
    try {
      final response = await ref
          .read(relaySessionProvider.notifier)
          .queryRelayJson([filter]);
      final rows = parseThreadSearchResponse(response);
      if (state.query != query) return;
      state = state.copyWith(threadResults: rows);
    } catch (error) {
      // A relay without chat state has no name search; the Chats rows on
      // this device still match. Other failures keep the earlier results.
      if (!isChatStateDisabledError(error)) {
        debugPrint('[Search] thread name search failed: $error');
      }
    }
  }

  Future<void> _searchMessages(String query) async {
    try {
      final session = ref.read(relaySessionProvider.notifier);
      // The HTTP bridge, not a REQ: only it honours prefix matching, which
      // lets "검색" find "검색창을".
      final events = await session.queryRelay([
        NostrFilters.searchMessages(query),
      ]);

      // Channel-name lookup from the cached channels list — not all hits
      // will have a known channel (e.g. ones we're not a member of).
      final channelsById = {
        for (final c in ref.read(channelsProvider).value ?? const <Channel>[])
          c.id: c.name,
      };

      final hits =
          events
              .map(
                (e) => SearchHit(
                  eventId: e.id,
                  content: e.content,
                  kind: e.kind,
                  pubkey: e.pubkey,
                  channelId: e.channelId,
                  channelName: e.channelId != null
                      ? channelsById[e.channelId!]
                      : null,
                  createdAt: e.createdAt,
                  score: 0.0,
                  tags: e.tags,
                ),
              )
              .toList()
            // Newest first.
            ..sort((a, b) => b.createdAt.compareTo(a.createdAt));

      if (state.query != query) return;
      state = state.copyWith(messageResults: hits, isLoading: false);
    } catch (e) {
      if (state.query != query) return;
      state = state.copyWith(isLoading: false, error: e.toString());
    }
  }

  Future<void> _searchUsers(String query) async {
    try {
      final users = await ref
          .read(channelActionsProvider)
          .searchUsers(query, limit: 8);

      if (state.query != query) return;
      state = state.copyWith(userResults: users);
    } catch (_) {
      // User search failure is non-critical — keep existing results.
    }
  }

  void clear() {
    _debounce?.cancel();
    state = const SearchState.initial();
  }
}

final searchProvider = NotifierProvider<SearchNotifier, SearchState>(
  SearchNotifier.new,
);
