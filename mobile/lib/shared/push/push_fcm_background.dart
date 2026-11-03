import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' show Locale, PlatformDispatcher;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:http/http.dart' as http;
import 'package:kuzz_android_push/kuzz_android_push.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:shared_preferences/shared_preferences.dart';

import '../community/community.dart';
import '../l10n/l10n.dart';
import '../l10n/locale_preference.dart'
    show localePreferenceKey, shippedLocaleForTag;
import '../relay/nip98.dart';
import '../relay/relay_provider.dart' show pubkeyFromNsec;
import '../relay/service_address.dart';
import 'push_inbox.dart' show isPushInboxToken;
import 'push_inbox_writer.dart';
import 'push_lease_matcher.dart';
import 'push_thread_target.dart';

/// The Android background push handler (stage B A3.3).
///
/// The push gateway sends a data-only `{"k":"wake"}` FCM message. With the
/// app off screen, KuzzPushMessagingService runs [runKuzzAndroidPushWake] in
/// a short-lived Flutter engine. Per push-enabled community it signs a
/// relay `/query` (NIP-98 with the device tag) with the inbox rules of the
/// iPhone notification extension, verifies and matches every event, keeps
/// them in the push inbox for the app's drain, then posts one notification
/// for the newest message. It never opens a socket and never touches the
/// message database.
///
/// - Nothing to show and every fetch answered: no notification (logged).
/// - A fetch failed and nothing else to show: a generic "New message".
/// - Age-restricted: the generic text only, and nothing is kept.

typedef PushAuthSigner =
    Future<String> Function({
      required String method,
      required String url,
      required List<int> bodyBytes,
      required String? nsec,
    });

/// What to post. [generic] notices carry no text: the caller fills in the
/// localized "New message".
class AndroidPushNotice {
  final bool generic;
  final String? title;
  final String? body;
  final String? subtitle;
  final String? eventId;
  final String? communityId;
  final String? channelId;

  /// Set for a thread reply: the tap opens this thread at [eventId].
  final String? threadRootId;

  /// The event's `created_at`: the app removes the notification once read.
  final int? createdAt;

  const AndroidPushNotice.generic()
    : generic = true,
      title = null,
      body = null,
      subtitle = null,
      eventId = null,
      communityId = null,
      channelId = null,
      threadRootId = null,
      createdAt = null;

  const AndroidPushNotice.message({
    required String this.title,
    required String this.body,
    this.subtitle,
    this.eventId,
    this.communityId,
    this.channelId,
    this.threadRootId,
    this.createdAt,
  }) : generic = false;

  Map<String, Object?> toChannelArguments(AppLocalizations strings) => {
    'id': generic ? 0 : (eventId ?? '').hashCode & 0x7fffffff,
    'channelName': strings.pushAndroidChannelName,
    'title': generic ? 'Kuzz' : title,
    'body': generic ? strings.pushAndroidGenericNotification : body,
    if (subtitle != null) 'subtitle': subtitle,
    if (eventId != null && communityId != null && channelId != null) ...{
      'eventId': eventId,
      'communityId': communityId,
      'channelId': channelId,
      if (isPushEventId(threadRootId)) 'threadRootId': threadRootId,
      if (createdAt != null) 'createdAt': createdAt,
    },
  };
}

class _ByteBudget {
  int left;
  _ByteBudget(this.left);

  bool take(int bytes) {
    if (bytes > left) {
      left = 0;
      return false;
    }
    left -= bytes;
    return true;
  }
}

typedef _Fetched = ({List<Map<String, dynamic>> matching, List<int> returned});

class _InboxFetch {
  final Community community;
  final String token;
  final List<Map<String, dynamic>> events;
  final int fetchedAt;
  final int? cursor;
  final int? nextCursor;

  _InboxFetch(
    this.community,
    this.token,
    this.events,
    this.fetchedAt,
    this.cursor,
    this.nextCursor,
  );
}

class _CommunityRun {
  final Map<String, dynamic>? candidate;
  final _InboxFetch? fetch;

  /// The query that decides the notification got no answer.
  final bool failed;

  _CommunityRun({this.candidate, this.fetch, required this.failed});
}

/// One wake run; see the library comment. Tests inject every input.
class AndroidPushWake {
  /// Rows per filter in the inbox fetch.
  static const inboxFetchLimit = 50;
  static const profileResponseBytes = 256 * 1024;

  final http.Client client;
  final Future<List<Community>> Function() loadCommunities;
  final Future<String?> Function() loadActiveCommunityId;
  final PushInboxWriter? inbox;
  final Future<bool> Function() isAgeRestricted;
  final PushAuthSigner sign;
  final DateTime Function() now;
  final int byteBudget;

  /// Ends of the inbox fetch, today's fallback query and the profile query,
  /// counted from the start of the run. All within FCM's ~10 s.
  final Duration boundedDeadline;
  final Duration fallbackDeadline;
  final Duration profileDeadline;

  AndroidPushWake({
    required this.client,
    required this.loadCommunities,
    required this.loadActiveCommunityId,
    required this.inbox,
    required this.isAgeRestricted,
    this.sign = buildNip98AuthHeader,
    DateTime Function()? now,
    this.byteBudget = 1048576,
    this.boundedDeadline = const Duration(seconds: 5),
    this.fallbackDeadline = const Duration(milliseconds: 7500),
    this.profileDeadline = const Duration(milliseconds: 8500),
  }) : now = now ?? DateTime.now;

  /// Communities whose pushes this phone can fetch: signed in with a local
  /// key, push on, and an accepted or desired policy.
  static bool eligible(Community community) =>
      !community.signedOut &&
      community.pushNotificationsEnabled &&
      !community.tokenAuth &&
      (community.nsec?.isNotEmpty ?? false) &&
      (_account(community)?.isNotEmpty ?? false) &&
      community.pushSubscriptionState.authoritative.isNotEmpty;

  Future<AndroidPushNotice?> run() async {
    final start = now();
    final communities = (await loadCommunities()).where(eligible).toList();
    if (communities.isEmpty) {
      debugPrint('[AndroidPush] wake: no push-enabled community');
      return null;
    }
    if (await isAgeRestricted()) return const AndroidPushNotice.generic();

    final budget = _ByteBudget(byteBudget);
    final runs = await Future.wait([
      for (final community in communities)
        _runCommunity(community, start, budget),
    ]);
    final fetches = [
      for (final run in runs)
        if (run.fetch != null) run.fetch!,
    ];
    ({Map<String, dynamic> event, Community community})? newest;
    for (var i = 0; i < runs.length; i++) {
      final candidate = runs[i].candidate;
      if (candidate == null) continue;
      if (newest == null || _newer(candidate, newest.event)) {
        newest = (event: candidate, community: communities[i]);
      }
    }
    if (newest == null) {
      await _keep(fetches, null, null);
      if (runs.any((run) => run.failed)) {
        return const AndroidPushNotice.generic();
      }
      debugPrint('[AndroidPush] wake: nothing new to show');
      return null;
    }

    final profile = await _profile(newest.event, newest.community, start);
    final activeId = await loadActiveCommunityId();
    // N10: a sender profile is kept only for the community on screen.
    await _keep(
      fetches,
      newest.community.id == activeId ? profile : null,
      newest.community.id,
    );
    final body = previewBody('${newest.event['content'] ?? ''}');
    if (body.isEmpty) return const AndroidPushNotice.generic();
    final senderName = profile == null ? null : profileDisplayName(profile);
    final channelId = _tagValue(newest.event, 'h');
    return AndroidPushNotice.message(
      title: senderName ?? newest.community.name,
      body: body,
      subtitle: senderName == null ? null : newest.community.name,
      eventId: newest.event['id'] as String,
      communityId: newest.community.id,
      channelId: channelId,
      threadRootId: pushThreadRootId(newest.event['tags'] as List),
      createdAt: newest.event['created_at'] as int?,
    );
  }

  Future<_CommunityRun> _runCommunity(
    Community community,
    DateTime start,
    _ByteBudget budget,
  ) async {
    final policies = community.pushSubscriptionState.authoritative;
    final url = queryUrl(community.relayUrl);
    final token = community.pushInboxToken;
    final today = [
      for (final policy in policies) {...policy.filter.toJson(), 'limit': 10},
    ];
    if (inbox == null || !isPushInboxToken(token)) {
      final fetched = await _fetch(
        community,
        url,
        today,
        _ByteBudget(byteBudget),
        start.add(fallbackDeadline),
      );
      return _CommunityRun(
        candidate: fetched == null
            ? null
            : newestMessage(fetched.matching, community),
        failed: fetched == null,
      );
    }

    final fetchedAt = now().millisecondsSinceEpoch ~/ 1000;
    final cursor = await inbox!.readCursor(token!);
    final since = PushInboxCursor.since(cursor: cursor, now: fetchedAt);
    final bounded = [
      for (final policy in policies)
        {...policy.filter.toJson(), 'since': since, 'limit': inboxFetchLimit},
    ];
    final result = await _fetch(
      community,
      url,
      bounded,
      budget,
      start.add(boundedDeadline),
    );
    final nextCursor = result == null
        ? cursor
        : PushInboxCursor.next(
            current: cursor,
            returned: result.returned,
            fallback: false,
          );
    final candidate = result == null
        ? null
        : newestMessage(result.matching, community);
    if (candidate != null) {
      return _CommunityRun(
        candidate: candidate,
        fetch: _InboxFetch(
          community,
          token,
          result!.matching,
          fetchedAt,
          cursor,
          nextCursor,
        ),
        failed: false,
      );
    }
    // Cut off, failed or nothing to show: today's query decides the
    // notification. Its events are kept too; it never moves the cursor.
    final fallback = await _fetch(
      community,
      url,
      today,
      _ByteBudget(byteBudget),
      start.add(fallbackDeadline),
    );
    final seen = <String>{};
    final kept = [
      ...?result?.matching,
      ...?fallback?.matching,
    ].where((event) => seen.add(event['id'] as String)).toList();
    return _CommunityRun(
      candidate: fallback == null
          ? null
          : newestMessage(fallback.matching, community),
      fetch: _InboxFetch(community, token, kept, fetchedAt, cursor, nextCursor),
      failed: fallback == null,
    );
  }

  /// Verified, policy-matching events of one query and the `created_at` of
  /// every verified row returned, or null when it got no usable answer.
  Future<_Fetched?> _fetch(
    Community community,
    String url,
    List<Map<String, dynamic>> filters,
    _ByteBudget budget,
    DateTime deadline,
  ) async {
    final rows = await _post(community, url, filters, budget, deadline);
    if (rows == null) return null;
    final policies = community.pushSubscriptionState.authoritative;
    final verified = rows.where(isVerifiedEvent).toList();
    return (
      matching: [
        for (final event in verified)
          if (pushPoliciesMatch(policies, event)) event,
      ],
      returned: [for (final event in verified) event['created_at'] as int],
    );
  }

  Future<List<Map<String, dynamic>>?> _post(
    Community community,
    String url,
    List<Map<String, dynamic>> filters,
    _ByteBudget budget,
    DateTime deadline,
  ) async {
    final remaining = deadline.difference(now());
    if (remaining <= Duration.zero) return null;
    Future<List<Map<String, dynamic>>?> attempt() async {
      final body = utf8.encode(jsonEncode(filters));
      final auth = await sign(
        method: 'POST',
        url: url,
        bodyBytes: body,
        nsec: community.nsec,
      );
      final request = http.Request('POST', Uri.parse(url))
        ..headers['Content-Type'] = 'application/json'
        ..headers['Authorization'] = auth
        ..bodyBytes = body;
      final response = await client.send(request);
      if (response.statusCode < 200 || response.statusCode >= 300) {
        debugPrint('[AndroidPush] query answered ${response.statusCode}');
        return null;
      }
      final bytes = BytesBuilder(copy: false);
      await for (final chunk in response.stream) {
        if (!budget.take(chunk.length)) {
          debugPrint('[AndroidPush] query over the byte budget');
          return null;
        }
        bytes.add(chunk);
      }
      final decoded = jsonDecode(utf8.decode(bytes.takeBytes()));
      if (decoded is! List) return null;
      return [
        for (final row in decoded)
          if (row is Map) Map<String, dynamic>.from(row),
      ];
    }

    try {
      return await attempt().timeout(remaining);
    } on TimeoutException {
      debugPrint('[AndroidPush] query cut off at the deadline');
      return null;
    } catch (error) {
      debugPrint('[AndroidPush] query failed: ${error.runtimeType}');
      return null;
    }
  }

  /// The sender's verified kind:0, if the relay has one in time.
  Future<Map<String, dynamic>?> _profile(
    Map<String, dynamic> event,
    Community community,
    DateTime start,
  ) async {
    final pubkey = event['pubkey'] as String;
    final rows = await _post(
      community,
      queryUrl(community.relayUrl),
      [
        {
          'kinds': [0],
          'authors': [pubkey],
          'limit': 1,
        },
      ],
      _ByteBudget(profileResponseBytes),
      start.add(profileDeadline),
    );
    Map<String, dynamic>? best;
    for (final row in rows ?? const <Map<String, dynamic>>[]) {
      if (row['kind'] != 0 || row['pubkey'] != pubkey) continue;
      if (!isVerifiedEvent(row)) continue;
      if (best == null || _newer(row, best)) best = row;
    }
    return best;
  }

  /// Writes the run's fetches: only for tokens still stored (re-read now)
  /// and never while age-restricted.
  Future<void> _keep(
    List<_InboxFetch> fetches,
    Map<String, dynamic>? profile,
    String? profileCommunityId,
  ) async {
    final writer = inbox;
    if (writer == null || fetches.isEmpty || await isAgeRestricted()) return;
    final live = {
      for (final community in await loadCommunities())
        if (!community.signedOut) community.pushInboxToken,
    };
    for (final fetch in fetches) {
      if (!live.contains(fetch.token)) continue;
      final events = [...fetch.events];
      if (profile != null &&
          fetch.community.id == profileCommunityId &&
          !events.any((event) => event['id'] == profile['id'])) {
        events.add(profile);
      }
      try {
        await writer.write(
          token: fetch.token,
          relayUrl: fetch.community.relayUrl,
          account: _account(fetch.community)!.toLowerCase(),
          fetchedAt: fetch.fetchedAt,
          events: events,
        );
        final next = fetch.nextCursor;
        if (next != null && next != fetch.cursor) {
          await writer.writeCursor(fetch.token, next);
        }
      } catch (error) {
        // The notification never depends on the inbox; the app's catch-up
        // fetches whatever was not kept.
        debugPrint('[AndroidPush] inbox write failed: ${error.runtimeType}');
      }
    }
  }
}

String? _account(Community community) {
  final pubkey = community.pubkey;
  if (pubkey != null && pubkey.isNotEmpty) return pubkey;
  return pubkeyFromNsec(community.nsec);
}

/// `https://host/query` for a stored `wss://host` relay URL, on this build's
/// current service address.
String queryUrl(String relayUrl) {
  final current = currentServiceUrl(relayUrl);
  final uri = Uri.parse(current);
  final scheme = switch (uri.scheme) {
    'wss' => 'https',
    'ws' => 'http',
    _ => uri.scheme,
  };
  return uri.replace(scheme: scheme).resolve('/query').toString();
}

/// Whether [event] is a well-formed Nostr event whose id and signature check.
bool isVerifiedEvent(Map<String, dynamic> event) {
  try {
    final id = event['id'];
    final pubkey = event['pubkey'];
    final createdAt = event['created_at'];
    final kind = event['kind'];
    final tags = event['tags'];
    final content = event['content'];
    final sig = event['sig'];
    if (id is! String ||
        pubkey is! String ||
        createdAt is! int ||
        kind is! int ||
        tags is! List ||
        content is! String ||
        sig is! String) {
      return false;
    }
    nostr.Event(
      id,
      pubkey,
      createdAt,
      kind,
      [
        for (final tag in tags)
          [for (final value in tag as List) value as String],
      ],
      content,
      sig,
    );
    return true;
  } catch (_) {
    return false;
  }
}

/// The newest message from someone else (Swift `newestMessage`).
Map<String, dynamic>? newestMessage(
  List<Map<String, dynamic>> events,
  Community community,
) {
  final mine = _account(community)?.toLowerCase();
  if (mine == null) return null;
  Map<String, dynamic>? newest;
  for (final event in events) {
    if ('${event['pubkey']}'.toLowerCase() == mine) continue;
    if (!const [9, 40002, 45001, 45003].contains(event['kind'])) continue;
    if (newest == null || _newer(event, newest)) newest = event;
  }
  return newest;
}

bool _newer(Map<String, dynamic> a, Map<String, dynamic> b) {
  final aAt = a['created_at'] as int;
  final bAt = b['created_at'] as int;
  if (aAt != bAt) return aAt > bAt;
  return (a['id'] as String).compareTo(b['id'] as String) < 0;
}

String? _tagValue(Map<String, dynamic> event, String name) {
  for (final tag in event['tags'] as List) {
    if (tag is List && tag.length > 1 && tag[0] == name) return '${tag[1]}';
  }
  return null;
}

/// The name in a kind:0, or null.
String? profileDisplayName(Map<String, dynamic> profile) {
  try {
    final content = jsonDecode(profile['content'] as String);
    if (content is! Map) return null;
    for (final key in const ['display_name', 'displayName', 'name']) {
      final value = content[key];
      if (value is String && value.trim().isNotEmpty) {
        final name = value.trim();
        return name.length > 64 ? '${name.substring(0, 63)}…' : name;
      }
    }
  } catch (_) {}
  return null;
}

final _codeBlock = RegExp(r'```[\s\S]*?```');
final _inlineCode = RegExp(r'`([^`]*)`');
final _markdownLink = RegExp(r'!?\[([^\]]*)\]\([^)]*\)');
final _bareLink = RegExp(r'https?://\S+');
final _space = RegExp(r'\s+');

/// Notification text for a message (Swift `previewBody`): code blocks become
/// `[code]`, links `[link]`, at most 180 characters.
String previewBody(String content) {
  var result = content.replaceAll(_codeBlock, '[code]');
  result = result.replaceAllMapped(_inlineCode, (match) => match[1]!);
  result = result.replaceAllMapped(_markdownLink, (match) => match[1]!);
  result = result.replaceAll(_bareLink, '[link]');
  result = result.replaceAll(_space, ' ').trim();
  final characters = result.runes.toList();
  if (characters.length <= 180) return result;
  return '${String.fromCharCodes(characters.take(177)).trim()}…';
}

const _communitiesKey = 'kuzz_communities';
const _activeCommunityKey = 'kuzz_active_community_id';

/// Stored communities, read only: the wake never migrates or rewrites the
/// list (`CommunityStorage.loadAll` may).
Future<List<Community>> readStoredCommunitiesForPush([
  FlutterSecureStorage storage = const FlutterSecureStorage(),
]) async {
  final raw = await storage.read(key: _communitiesKey);
  if (raw == null) return const [];
  final list = jsonDecode(raw) as List<dynamic>;
  return [
    for (final entry in list)
      Community.fromJson(Map<String, dynamic>.from(entry as Map)),
  ];
}

/// Where Android keeps the age-restriction mark next to `push-inbox/`.
String androidPushAgeMarkerPath(String inboxRoot) =>
    '${File(inboxRoot).parent.path}/push-age-restricted';

/// The app's language: the one picked in Settings, else the phone's.
Future<AppLocalizations> backgroundPushStrings() async {
  Locale? picked;
  try {
    final prefs = await SharedPreferences.getInstance();
    picked = shippedLocaleForTag(prefs.getString(localePreferenceKey));
  } catch (_) {
    // No preferences: follow the phone.
  }
  return lookupAppLocalizations(
    picked ?? resolveAppLocale(PlatformDispatcher.instance.locales),
  );
}

/// The wake engine's work: resolve, post, and tell native it is done.
Future<void> runKuzzAndroidPushWake() async {
  const channel = MethodChannel(kuzzAndroidPushChannel);
  AndroidPushNotice? notice;
  try {
    final root = await channel.invokeMethod<String>('pushInboxRoot');
    final client = http.Client();
    try {
      notice =
          await AndroidPushWake(
            client: client,
            loadCommunities: readStoredCommunitiesForPush,
            loadActiveCommunityId: () =>
                const FlutterSecureStorage().read(key: _activeCommunityKey),
            inbox: root == null ? null : PushInboxWriter(root),
            isAgeRestricted: () async =>
                root != null &&
                File(androidPushAgeMarkerPath(root)).existsSync(),
          ).run().timeout(
            const Duration(seconds: 9),
            onTimeout: () => const AndroidPushNotice.generic(),
          );
    } finally {
      client.close();
    }
  } catch (error) {
    debugPrint('[AndroidPush] wake failed: ${error.runtimeType}');
    notice = const AndroidPushNotice.generic();
  }
  try {
    if (notice != null) {
      final strings = await backgroundPushStrings();
      await channel.invokeMethod<void>(
        'show',
        notice.toChannelArguments(strings),
      );
    }
  } catch (error) {
    debugPrint('[AndroidPush] notification failed: ${error.runtimeType}');
  } finally {
    await channel.invokeMethod<void>('wakeDone');
  }
}
