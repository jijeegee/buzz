import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chat_list_provider.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:kuzz/shared/push/push_inbox_events.dart';
import 'package:kuzz/shared/read_state/read_state_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';

const _relay = 'wss://relay.example';
const _listed = '11111111-1111-4111-8111-111111111111';
const _other = '22222222-2222-4222-8222-222222222222';
final _me = 'a' * 64;
final _them = 'b' * 64;

class _Config extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: _relay);
}

/// Offline until [connect]; answers `chat_list` with [pages] in order.
class _Session extends RelaySessionNotifier {
  final List<Object?> pages = [];
  int queries = 0;

  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);

  void connect() => state = const SessionState(status: SessionStatus.connected);

  @override
  Future<Object?> queryRelayJson(
    List<Map<String, dynamic>> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    queries++;
    if (pages.isEmpty) throw StateError('no page left');
    return pages.removeAt(0);
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
    void Function()? onGap,
  }) async => () {};
}

class _ReadState extends ReadStateNotifier {
  _ReadState(this.contexts);

  final Map<String, int> contexts;

  @override
  ReadStateState build() => ReadStateState(
    isReady: true,
    pubkey: _me,
    contexts: Map.unmodifiable(contexts),
    version: 1,
  );
}

NostrEvent _event(String id, String room, int at) => NostrEvent(
  id: id,
  pubkey: _them,
  createdAt: at,
  kind: 9,
  tags: [
    ['h', room],
  ],
  content: id,
  sig: '',
);

ChatListRow _room(
  String channelId,
  int at,
  NostrEvent latest, {
  int unread = 0,
}) => ChatListRow(
  isThread: false,
  key: channelId,
  at: at,
  atUs: at * 1000000,
  channelId: channelId,
  channelType: 'dm',
  channelName: 'dm',
  unread: unread,
  latest: latest,
);

String _page(List<ChatListRow> rows) => jsonEncode({
  'chat_list': 1,
  'rows': [for (final row in rows) row.toJson()],
});

/// Chats shows the push inbox's events at once: saved rows drawn at a cold
/// start take the events drained before, and rows shown take each drain
/// (docs/message-sync.md#push-inbox).
void main() {
  late ProviderContainer container;
  late SharedPreferences prefs;
  late String cacheKey;
  late String saved;
  late _Session session;

  Future<void> start({Map<String, int> read = const {}}) async {
    saved = _page([
      _room(_listed, 10, _event('l0', _listed, 10)),
      _room(_other, 5, _event('o0', _other, 5)),
    ]);
    SharedPreferences.setMockInitialValues({});
    prefs = await SharedPreferences.getInstance();
    session = _Session();
    container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        relayConfigProvider.overrideWith(_Config.new),
        myPubkeyProvider.overrideWithValue(_me),
        relaySessionProvider.overrideWith(() => session),
        readStateProvider.overrideWith(() => _ReadState(read)),
      ],
    );
    addTearDown(container.dispose);
    container.read(relaySessionProvider);
    final baseUrl = container.read(relayConfigProvider).baseUrl;
    cacheKey = 'kuzz.chatList.v1:${storageScopeFor(baseUrl)}:$_me';
    await prefs.setString(cacheKey, saved);
  }

  String owner() => container.read(chatListOwnerProvider)!;
  MessageStoreLease lease() =>
      container.read(messageStoreProvider).lease(_relay, _me)!;
  PushedChatEvents pushed() => container.read(pushedChatEventsProvider);

  Future<ChatListNotifier> shown() async {
    container.listen(chatListForOwnerProvider(owner()), (_, _) {});
    await container.read(chatListForOwnerProvider(owner()).future);
    return container.read(chatListForOwnerProvider(owner()).notifier);
  }

  ChatListState current() =>
      container.read(chatListForOwnerProvider(owner())).requireValue;
  Map<String, ChatListRow> rows() => {
    for (final row in current().rows) row.channelId: row,
  };

  test('a cold start draws the saved rows with events drained before, '
      'once, unsaved', () async {
    await start();
    // The drain ran before the list was first read, before any socket.
    pushed().add(lease(), [
      // As old as the saved latest: the saved count already has it.
      _event('l-same-second', _listed, 10),
      _event('l1', _listed, 20),
    ]);
    final chats = await shown();
    expect(current().fromCache, isTrue);
    expect(rows()[_listed]!.latest!.id, 'l1');
    expect(rows()[_listed]!.unread, 1);
    expect(current().rows.first.channelId, _listed);

    // The same events again (a later drain) count once.
    chats.foldPushed(lease(), [_event('l1', _listed, 20)]);
    expect(rows()[_listed]!.unread, 1);
    expect(rows()[_other]!.latest!.id, 'o0');
    expect(prefs.getString(cacheKey), saved, reason: 'nothing is saved');
  });

  test('an event this device has read past moves the row, uncounted', () async {
    await start(read: {_listed: 25});
    pushed().add(lease(), [_event('l1', _listed, 20)]);
    await shown();
    expect(rows()[_listed]!.latest!.id, 'l1');
    expect(rows()[_listed]!.unread, 0);
  });

  test(
    'the first page that has the event counts it once and drops it',
    () async {
      await start();
      pushed().add(lease(), [_event('l1', _listed, 20)]);
      await shown();
      expect(rows()[_listed]!.unread, 1);

      session.pages.add(
        jsonDecode(
          _page([
            _room(_listed, 20, _event('l1', _listed, 20), unread: 1),
            _room(_other, 5, _event('o0', _other, 5)),
          ]),
        ),
      );
      session.connect();
      await container.read(chatListForOwnerProvider(owner()).future);
      await pumpEventQueue();
      expect(current().fromCache, isFalse);
      expect(rows()[_listed]!.latest!.id, 'l1');
      expect(rows()[_listed]!.unread, 1);
      expect(pushed().after(lease()), isEmpty, reason: 'the page had it');
      expect(
        jsonDecode(prefs.getString(cacheKey)!)['rows'][0]['latest']['id'],
        'l1',
        reason: 'only the relay page is saved',
      );
    },
  );

  test('a live list takes a drain at once, without reloading', () async {
    await start();
    session.pages.add(jsonDecode(saved));
    session.connect();
    final chats = await shown();
    await pumpEventQueue();
    expect(current().fromCache, isFalse);
    final queries = session.queries;

    chats.foldPushed(lease(), [_event('o1', _other, 30)]);
    expect(current().rows.first.channelId, _other);
    expect(rows()[_other]!.latest!.id, 'o1');
    expect(rows()[_other]!.unread, 1);
    await pumpEventQueue();
    expect(session.queries, queries, reason: 'no reload');
  });

  test('another lease folds nothing', () async {
    await start();
    final chats = await shown();
    final other = MessageStore(
      factory: () => throw UnimplementedError(),
      directory: () async => '',
    ).lease(_relay, 'c' * 64)!;
    chats.foldPushed(other, [_event('l1', _listed, 20)]);
    expect(rows()[_listed]!.latest!.id, 'l0');
    // Nor are its kept events drawn for this one.
    pushed().add(other, [_event('l2', _listed, 30)]);
    expect(pushed().after(lease()), isEmpty);
  });
}
