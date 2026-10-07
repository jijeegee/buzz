import 'package:flutter/foundation.dart';

import 'timeline_message.dart';

/// NIP-18 quote tag name: `["q", <event id>, <relay url or "">, <author>]`.
///
/// A quote is a reference only. It never decides where a message lands: the
/// `e` reply tags alone place a message on the main timeline or in a thread.
/// Mirrors desktop's `messageQuote.ts`.
const quoteTagName = 'q';

/// Longest one-line excerpt shown for a quoted message (desktop parity).
const _quoteExcerptMaxLength = 160;

final _hex64 = RegExp(r'^[0-9a-f]{64}$');

String? _normalizeHex(String? value) {
  final normalized = value?.trim().toLowerCase() ?? '';
  return _hex64.hasMatch(normalized) ? normalized : null;
}

/// The event a composer will quote on its next send.
@immutable
class QuoteTarget {
  /// Lowercase hex event id of the quoted message.
  final String eventId;

  /// Lowercase hex pubkey of the quoted author (the `q` tag's 4th value).
  final String authorPubkey;

  /// Display name of the quoted author, for the composer chip.
  final String author;

  /// One-line plain-text excerpt of the quoted message.
  final String excerpt;

  const QuoteTarget({
    required this.eventId,
    required this.authorPubkey,
    required this.author,
    required this.excerpt,
  });

  @override
  bool operator ==(Object other) =>
      other is QuoteTarget &&
      other.eventId == eventId &&
      other.authorPubkey == authorPubkey &&
      other.author == author &&
      other.excerpt == excerpt;

  @override
  int get hashCode => Object.hash(eventId, authorPubkey, author, excerpt);
}

/// A well-formed `q` tag read back from a message.
@immutable
class QuoteReference {
  final String eventId;
  final String? authorPubkey;

  const QuoteReference({required this.eventId, this.authorPubkey});

  @override
  bool operator ==(Object other) =>
      other is QuoteReference &&
      other.eventId == eventId &&
      other.authorPubkey == authorPubkey;

  @override
  int get hashCode => Object.hash(eventId, authorPubkey);
}

/// Builds the single NIP-18 quote tag for [eventId] by [authorPubkey].
///
/// Both values must be 64-character hex; they are trimmed and lowercased.
List<String> buildQuoteTag({
  required String eventId,
  required String authorPubkey,
}) {
  final normalizedEventId = _normalizeHex(eventId);
  if (normalizedEventId == null) {
    throw ArgumentError.value(eventId, 'eventId', 'must be 64 hex characters');
  }
  final normalizedAuthor = _normalizeHex(authorPubkey);
  if (normalizedAuthor == null) {
    throw ArgumentError.value(
      authorPubkey,
      'authorPubkey',
      'must be 64 hex characters',
    );
  }
  return [quoteTagName, normalizedEventId, '', normalizedAuthor];
}

/// The first well-formed `q` tag in [tags], if any.
QuoteReference? quoteReferenceOf(List<List<String>> tags) {
  for (final tag in tags) {
    if (tag.length < 2 || tag[0] != quoteTagName) continue;
    final eventId = _normalizeHex(tag[1]);
    if (eventId == null) continue;
    return QuoteReference(
      eventId: eventId,
      authorPubkey: tag.length >= 4 ? _normalizeHex(tag[3]) : null,
    );
  }
  return null;
}

/// Ordinary messages with a hex id and author can be quoted.
bool canQuoteMessage(TimelineMessage message) =>
    !message.isSystem &&
    _normalizeHex(message.id) != null &&
    _normalizeHex(message.pubkey) != null;

/// Quote target for [message], labelled [author]; null when not quotable.
QuoteTarget? quoteTargetFor(TimelineMessage message, {required String author}) {
  if (!canQuoteMessage(message)) return null;
  return QuoteTarget(
    eventId: _normalizeHex(message.id)!,
    authorPubkey: _normalizeHex(message.pubkey)!,
    author: author,
    excerpt: quoteExcerpt(message.content),
  );
}

/// One-line plain-text summary of message [content] (desktop's
/// `summarizeMessageLinkContent`).
String quoteExcerpt(String content) {
  final normalized = content
      .replaceAll(RegExp(r'[\u0000-\u001f\u007f-\u009f]'), ' ')
      .replaceAll(RegExp(r'\|\|[^|]*(?:\|(?!\|)[^|]*)*\|\|'), ' ')
      .replaceAll(RegExp(r'!\[[^\]]*\]\([^)]*\)'), ' ')
      .replaceAllMapped(
        RegExp(r'\[([^\]]+)\]\([^)]*\)'),
        (match) => match.group(1)!,
      )
      .replaceAll(RegExp(r'<?(?:https?|buzz)://\S+>?'), ' ')
      .replaceAll(RegExp(r'[`*_~>#|]'), ' ')
      .replaceAll(RegExp(r'\s+'), ' ')
      .trim();
  if (normalized.isEmpty) return 'No message text';

  final characters = normalized.runes.toList();
  if (characters.length <= _quoteExcerptMaxLength) return normalized;
  final clipped = String.fromCharCodes(
    characters.take(_quoteExcerptMaxLength - 1),
  );
  final lastSpace = clipped.lastIndexOf(' ');
  final snippet = lastSpace > 96 ? clipped.substring(0, lastSpace) : clipped;
  return '${snippet.trimRight()}…';
}
