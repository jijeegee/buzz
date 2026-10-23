import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';

/// One quote box: the message [quotingId] in [channelId] quotes [quotedId].
typedef QuotePreviewKey = ({
  String channelId,
  String quotedId,
  String quotingId,
});

/// A relay-cut excerpt of a quoted message sent before I joined the room.
///
/// The original stays hidden; the relay shows only this much of it, and only
/// inside the quote box of a message I can see.
@immutable
class QuotePreview {
  /// Lowercase hex pubkey of the quoted message's author.
  final String authorPubkey;

  /// Plain-text excerpt cut by the relay ("Photo"/"File" for attachments).
  final String excerpt;

  const QuotePreview({required this.authorPubkey, required this.excerpt});
}

/// The kind:39008 filter for [key] (`/query` bridge extension).
Map<String, Object?> quotePreviewFilter(QuotePreviewKey key) => {
  'quote_preview': true,
  '#h': [key.channelId],
  'ids': [key.quotedId],
  '#e': [key.quotingId],
};

/// Reads the relay's kind:39008 answer for [key]; null when there is none or
/// it does not describe this quote.
QuotePreview? quotePreviewFromEvents(
  Iterable<NostrEvent> events,
  QuotePreviewKey key,
) {
  for (final event in events) {
    if (event.kind != EventKind.quotePreview) continue;
    if (event.getTagValue('e')?.toLowerCase() != key.quotedId) continue;
    if (event.getTagValue('h') != key.channelId) continue;
    final author = event.getTagValue('p')?.trim().toLowerCase();
    if (author == null || author.isEmpty) continue;
    final Object? content;
    try {
      content = jsonDecode(event.content);
    } on FormatException {
      continue;
    }
    if (content is! Map || content['excerpt'] is! String) continue;
    return QuotePreview(
      authorPubkey: author,
      excerpt: content['excerpt'] as String,
    );
  }
  return null;
}

/// Asks the relay for the excerpt of a quoted message I can't read because it
/// was sent before I joined. Resolves to null when the relay has none (no
/// access, not a quote, deleted, or I could read it anyway).
///
/// A settled answer stays cached per quote for the session; a failed request
/// is retried the next time the quote box shows.
final quotePreviewProvider = FutureProvider.autoDispose
    .family<QuotePreview?, QuotePreviewKey>((ref, key) async {
      // A different relay or account asks again.
      ref.watch(relayConfigProvider);
      final retention = ref.keepAlive();
      try {
        final decoded = await ref
            .read(relaySessionProvider.notifier)
            .queryRelayJson([quotePreviewFilter(key)]);
        if (decoded is! List) {
          throw const FormatException('relay returned malformed quote preview');
        }
        return quotePreviewFromEvents([
          for (final json in decoded)
            if (json is Map<String, dynamic>) NostrEvent.fromJson(json),
        ], key);
      } catch (_) {
        retention.close();
        rethrow;
      }
    });
