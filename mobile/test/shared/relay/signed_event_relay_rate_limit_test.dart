import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:nostr/nostr.dart' as nostr;

/// Back-pressure is on until [waitForRateLimit] returns.
class _HeldSession extends RelaySessionNotifier {
  _HeldSession({required this.limited});

  bool limited;
  final published = <NostrEvent>[];

  @override
  bool get isRateLimited => limited;

  @override
  Future<void> waitForRateLimit() async {
    // Signing uses whole seconds: let one pass.
    await Future<void>.delayed(const Duration(milliseconds: 1100));
    limited = false;
  }

  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    published.add(event);
    return event;
  }
}

String _nsec() => nostr.Keys.generate().nsec;

void main() {
  test('a message held by back-pressure is signed again when it goes out',
      () async {
    final session = _HeldSession(limited: true);
    final relay = SignedEventRelay(session: session, nsec: _nsec());
    final signed = <NostrEvent>[];

    await relay.submit(
      kind: 9,
      content: 'hi',
      tags: const [],
      onSigned: signed.add,
    );

    expect(signed, hasLength(2));
    expect(signed.last.createdAt, greaterThan(signed.first.createdAt));
    // Only the last one is sent.
    expect(session.published.map((event) => event.id), [signed.last.id]);
  });

  test('a message with a set time is never signed again', () async {
    final session = _HeldSession(limited: true);
    final relay = SignedEventRelay(session: session, nsec: _nsec());
    final signed = <NostrEvent>[];

    await relay.submit(
      kind: 9,
      content: 'retry',
      tags: const [],
      createdAt: 1000,
      onSigned: signed.add,
    );

    expect(signed, hasLength(1));
    expect(session.published.single.createdAt, 1000);
  });
}
