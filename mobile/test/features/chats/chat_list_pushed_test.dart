import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chat_list_provider.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';

const _relay = 'wss://relay.example';
const _listed = '11111111-1111-4111-8111-111111111111';
const _hidden = '22222222-2222-4222-8222-222222222222';
final _me = 'a' * 64;
final _them = 'b' * 64;

class _Config extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: _relay);
}

class _Offline extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
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

ChatListRow _room(String channelId, int at, NostrEvent latest) => ChatListRow(
  isThread: false,
  key: channelId,
  at: at,
  atUs: at * 1000000,
  channelId: channelId,
  channelType: 'dm',
  channelName: 'dm',
  latest: latest,
);

/// Chats folds push inbox events into the saved rows only
/// (docs/message-sync.md#push-inbox).
void main() {
  late ProviderContainer container;
  late SharedPreferences prefs;
  late String cacheKey;
  late String saved;

  setUp(() async {
    saved = jsonEncode({
      'chat_list': 1,
      'rows': [
        _room(_listed, 10, _event('l0', _listed, 10)).toJson(),
        _room(_hidden, 5, _event('h0', _hidden, 5)).toJson(),
      ],
    });
    SharedPreferences.setMockInitialValues({});
    prefs = await SharedPreferences.getInstance();
    container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        relayConfigProvider.overrideWith(_Config.new),
        myPubkeyProvider.overrideWithValue(_me),
        relaySessionProvider.overrideWith(_Offline.new),
      ],
    );
    addTearDown(container.dispose);
    final baseUrl = container.read(relayConfigProvider).baseUrl;
    cacheKey = 'kuzz.chatList.v1:${storageScopeFor(baseUrl)}:$_me';
    await prefs.setString(cacheKey, saved);
  });

  String owner() => container.read(chatListOwnerProvider)!;
  MessageStoreLease lease() =>
      container.read(messageStoreProvider).lease(_relay, _me)!;

  Future<ChatListNotifier> shown() async {
    container.listen(chatListForOwnerProvider(owner()), (_, _) {});
    final state = await container.read(
      chatListForOwnerProvider(owner()).future,
    );
    expect(state.fromCache, isTrue);
    return container.read(chatListForOwnerProvider(owner()).notifier);
  }

  ChatListState current() =>
      container.read(chatListForOwnerProvider(owner())).requireValue;

  test(
    'saved rows of listed rooms take pushed events, once, unsaved',
    () async {
      final chats = await shown();
      final events = [
      // As old as the saved latest: the saved count already has it.
      _event('l-same-second', _listed, 10),
      _event('l1', _listed, 20),
      _event('h1', _hidden, 30),
    ];
      chats.foldPushed(lease(), events, listed: {_listed});
      // The same events again (a second drain, a replay) count once.
      chats.foldPushed(lease(), events, listed: {_listed});

      final rows = {for (final row in current().rows) row.channelId: row};
      expect(rows[_listed]!.latest!.id, 'l1');
      expect(rows[_listed]!.unread, 1);
      expect(rows[_hidden]!.latest!.id, 'h0', reason: 'a hidden DM stays');
      expect(rows[_hidden]!.unread, 0);
      expect(current().fromCache, isTrue);
      expect(prefs.getString(cacheKey), saved, reason: 'nothing is saved');
    },
  );

  test('a live list or an ended lease folds nothing', () async {
    final chats = await shown();
    final other = MessageStore(
      factory: () => throw UnimplementedError(),
      directory: () async => '',
    ).lease(_relay, 'c' * 64)!;
    chats.foldPushed(other, [_event('l1', _listed, 20)], listed: {_listed});
    expect(current().rows.first.latest!.id, 'l0');

    final live = current().copyWith(fromCache: false);
    container.read(chatListForOwnerProvider(owner()).notifier).state =
        AsyncData(live);
    chats.foldPushed(lease(), [_event('l1', _listed, 20)], listed: {_listed});
    expect(current().rows.first.latest!.id, 'l0');
  });
}
