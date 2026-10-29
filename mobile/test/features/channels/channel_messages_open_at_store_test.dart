import 'dart:async';

import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/features/channels/channel_messages_provider.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'window_relay_fixture.dart';

// Opening a room at a message from a certain stored window does not wait for
// the relay's newest page (docs/message-sync.md#opening-at-a-position).

const _channelId = '11111111-1111-4111-8111-111111111111';

NostrEvent _row(int n) =>
    fixtureRow(_channelId, (100000 - n) * 16, 1000 + n ~/ 3);

bool _isHead(NostrFilter filter) =>
    filter.until == null && filter.extensions['after_id'] == null;

/// Serves NIP-CW windows over [rows]; the newest page waits for [headGate].
class _GatedRelaySession extends RelaySessionNotifier {
  _GatedRelaySession(this.rows);

  final List<NostrEvent> rows;
  final headGate = Completer<void>();
  final queries = <NostrFilter>[];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final filter = filters.single;
    queries.add(filter);
    if (_isHead(filter)) await headGate.future;
    return serveChannelWindow(_channelId, rows, filter);
  }

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async => const [];

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async =>
      () {};
}

ProviderContainer _container(
  _GatedRelaySession session,
  List<NostrEvent> stored, {
  bool supportsNewer = true,
}) {
  final container = ProviderContainer(
    overrides: [
      relaySessionProvider.overrideWith(() => session),
      channelHeadScopeProvider.overrideWithValue(
        const ChannelHeadScope(relayUrl: 'wss://r', pubkey: 'me'),
      ),
      storedRoomHeadProvider.overrideWithValue(
        (session, roomId, filter) async =>
            serveChannelWindow(_channelId, stored, filter),
      ),
      relayInfoProvider.overrideWith(
        (ref) => Future.value(
          RelayInfo(
            supportedExtensions: supportsNewer ? {newerWindowExtension} : {},
          ),
        ),
      ),
    ],
  );
  addTearDown(container.dispose);
  return container;
}

Future<void> _settle() async {
  for (var i = 0; i < 5; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

List<String> _shown(ProviderContainer container) => container
    .read(channelMessagesProvider(_channelId))
    .value!
    .map((event) => event.id)
    .toList();

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test(
    'opening at a message from the store does not wait for the newest page',
    () async {
      final rows = [for (var n = 1; n <= 300; n++) _row(n)];
      final session = _GatedRelaySession(rows);
      // The store's window is a little behind the relay.
      final container = _container(session, rows.sublist(0, 290));
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      final notifier = container.read(
        channelMessagesProvider(_channelId).notifier,
      );
      final anchor = rows[99];

      expect(
        await notifier.openAt(createdAt: anchor.createdAt, eventId: anchor.id),
        isTrue,
      );
      // The newest page was asked for once and is still out; the window
      // opened at the message took its own two pages.
      expect(session.queries.where(_isHead), hasLength(1));
      expect(session.queries, hasLength(3));
      expect(notifier.isInMiddleWindow, isTrue);
      final opened = rows.sublist(50, 150).map((event) => event.id).toList();
      expect(_shown(container), opened);

      // The late newest page does not replace the window opened first.
      session.headGate.complete();
      await _settle();
      expect(notifier.isInMiddleWindow, isTrue);
      expect(notifier.anchorId, anchor.id);
      expect(_shown(container), opened);
      expect(session.queries, hasLength(3));
    },
  );

  test(
    'when the window cannot open at the message the newest page still lands',
    () async {
      final rows = [for (var n = 1; n <= 300; n++) _row(n)];
      final session = _GatedRelaySession(rows);
      final container = _container(
        session,
        rows.sublist(0, 290),
        supportsNewer: false,
      );
      container.listen(channelMessagesProvider(_channelId), (_, _) {});
      final notifier = container.read(
        channelMessagesProvider(_channelId).notifier,
      );

      expect(await notifier.canOpenAt(), isFalse);
      expect(
        await notifier.openAt(
          createdAt: rows[99].createdAt,
          eventId: rows[99].id,
        ),
        isFalse,
      );
      session.headGate.complete();
      await _settle();
      expect(notifier.isInMiddleWindow, isFalse);
      // The relay's newest window replaced the stored one.
      expect(_shown(container).last, rows.last.id);
      expect(session.queries, hasLength(1));
    },
  );
}
