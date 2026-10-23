import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/quote_preview_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _channelId = '11111111-1111-4111-8111-111111111111';
final _quoted = 'c' * 64;
final _quoting = 'e' * 64;
final _author = 'a' * 64;
final _key = (channelId: _channelId, quotedId: _quoted, quotingId: _quoting);

NostrEvent _preview({
  int kind = EventKind.quotePreview,
  String? quoted,
  String channel = _channelId,
  String? author,
  Object? content,
}) => NostrEvent(
  id: 'f' * 64,
  pubkey: 'relay',
  createdAt: 1,
  kind: kind,
  tags: [
    ['e', quoted ?? _quoted],
    ['d', quoted ?? _quoted],
    ['h', channel],
    if (author != '') ['p', author ?? _author],
  ],
  content: content is String
      ? content
      : jsonEncode(
          content ?? {'excerpt': 'Hello there', 'kind': 9, 'created_at': 1},
        ),
  sig: 'sig',
);

void main() {
  test('asks for one quote with the quote_preview extension', () {
    expect(quotePreviewFilter(_key), {
      'quote_preview': true,
      '#h': [_channelId],
      'ids': [_quoted],
      '#e': [_quoting],
    });
  });

  test('reads the author and excerpt of a matching preview', () {
    final preview = quotePreviewFromEvents([_preview()], _key);
    expect(preview?.authorPubkey, _author);
    expect(preview?.excerpt, 'Hello there');
  });

  test('ignores answers that do not describe this quote', () {
    for (final event in [
      _preview(kind: EventKind.streamMessage),
      _preview(quoted: 'b' * 64),
      _preview(channel: '22222222-2222-4222-8222-222222222222'),
      _preview(author: ''),
      _preview(content: 'not json'),
      _preview(content: {'kind': 9}),
    ]) {
      expect(quotePreviewFromEvents([event], _key), isNull);
    }
    expect(quotePreviewFromEvents(const [], _key), isNull);
  });
}
