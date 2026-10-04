import 'dart:async';

import 'package:nostr/nostr.dart' as nostr;

import 'nostr_models.dart';
import 'relay_provider.dart';
import 'relay_session.dart';
import 'relay_socket.dart';

/// The pubkey this community's outgoing events are authored as: the token
/// principal on a token community (the relay stamps drafts as it), else the
/// one derived from the nsec. `null` when there is no identity to publish as.
String? outgoingAuthorPubkey(RelayConfig config) {
  if (config.tokenAuth) {
    final principal = config.principalId;
    return principal == null || principal.isEmpty ? null : principal;
  }
  return pubkeyFromNsec(config.nsec);
}

/// An unsigned event draft for a token-authenticated socket (plan §3.2).
///
/// The relay stamps `pubkey` (it must equal the principal), ignores `sig`,
/// and recomputes the NIP-01 id. The id is still computed locally so the
/// relay's `OK` (and any rejection) correlates with the pending publish.
NostrEvent unsignedDraft({
  required String principalId,
  required int kind,
  required String content,
  required List<List<String>> tags,
  int? createdAt,
}) {
  final event = nostr.Event.unsigned(
    pubkey: principalId,
    kind: kind,
    content: content,
    tags: tags,
    createdAt: createdAt,
  );
  return NostrEvent(
    id: event.id,
    pubkey: event.pubkey,
    createdAt: event.createdAt,
    kind: event.kind,
    tags: event.tags,
    content: event.content,
  );
}

/// The event this community publishes: an [unsignedDraft] on a token
/// community, otherwise signed with the nsec. Throws when the community has
/// no identity to publish as (no principal / no usable key).
NostrEvent buildOutgoingEvent(
  RelayConfig config, {
  required int kind,
  required String content,
  required List<List<String>> tags,
  int? createdAt,
}) {
  if (config.tokenAuth) {
    final principal = config.principalId;
    if (principal == null || principal.isEmpty) {
      throw StateError('Token community has no principal id');
    }
    return unsignedDraft(
      principalId: principal,
      kind: kind,
      content: content,
      tags: tags,
      createdAt: createdAt,
    );
  }
  return _signEvent(
    nsec: config.nsec,
    kind: kind,
    content: content,
    tags: tags,
    createdAt: createdAt,
  );
}

NostrEvent _signEvent({
  required String? nsec,
  required int kind,
  required String content,
  required List<List<String>> tags,
  int? createdAt,
}) {
  if (nsec == null || nsec.isEmpty) {
    throw Exception('Cannot submit event: no signing key available');
  }
  final privkeyHex = nostr.Nip19.decode(payload: nsec).data;
  if (privkeyHex.isEmpty) {
    throw Exception('Invalid nsec');
  }
  final event = nostr.Event.from(
    kind: kind,
    content: content,
    tags: tags,
    secretKey: privkeyHex,
    createdAt: createdAt,
    verify: false,
  );
  return NostrEvent.fromJson(event.toMap());
}

/// Submits events through the relay WebSocket connection: signed with the
/// nsec on a legacy community, as unsigned drafts on a token community.
class SignedEventRelay {
  final RelaySessionNotifier _session;
  final RelayConfig _config;

  /// Legacy (nsec-signing) relay.
  SignedEventRelay({
    required RelaySessionNotifier session,
    required String? nsec,
  }) : _session = session,
       _config = RelayConfig(baseUrl: '', nsec: nsec);

  /// The relay for the active community [config] — drafts in token mode.
  SignedEventRelay.forConfig({
    required RelaySessionNotifier session,
    required RelayConfig config,
  }) : _session = session,
       _config = config;

  /// The hex pubkey events are authored as, or null if there is none.
  String? get pubkey => outgoingAuthorPubkey(_config);

  /// Sign (or draft) and submit an event. Returns the relay's OK response as
  /// a [NostrEvent] whose `content` field contains the OK message (e.g.
  /// `"response:{...}"` for command kinds).
  Future<NostrEvent> submit({
    required int kind,
    required String content,
    required List<List<String>> tags,
    int? createdAt,
    void Function(NostrEvent event)? onSigned,
  }) async {
    final event = buildOutgoingEvent(
      _config,
      kind: kind,
      content: content,
      tags: tags,
      createdAt: createdAt,
    );
    onSigned?.call(event);
    return _session.publish(event);
  }
}

/// Publishes one signed event over a short-lived authenticated NIP-42 socket.
///
/// This is used for community-removal tombstones because the community being
/// removed is not necessarily the app's active relay session.
Future<NostrEvent> submitSignedEventOnce({
  required String wsUrl,
  required String nsec,
  required int kind,
  required String content,
  required List<List<String>> tags,
  int? createdAt,
  Duration timeout = const Duration(seconds: 12),
}) async {
  final privateKey = nostr.Nip19.decode(payload: nsec).data;
  if (privateKey.isEmpty) throw const FormatException('Invalid nsec');
  final signed = nostr.Event.from(
    kind: kind,
    content: content,
    tags: tags,
    secretKey: privateKey,
    createdAt: createdAt,
    verify: false,
  );
  final event = NostrEvent.fromJson(signed.toMap());
  final result = Completer<NostrEvent>();
  late final RelaySocket socket;
  socket = RelaySocket(
    wsUrl: wsUrl,
    nsec: nsec,
    onMessage: (message) {
      if (message case [
        'OK',
        final String eventId,
        final bool accepted,
        final String detail,
        ...,
      ] when eventId == event.id) {
        if (accepted) {
          result.complete(
            NostrEvent(
              id: event.id,
              pubkey: event.pubkey,
              createdAt: event.createdAt,
              kind: event.kind,
              tags: event.tags,
              content: detail,
              sig: event.sig,
            ),
          );
        } else {
          result.completeError(Exception('Relay rejected event: $detail'));
        }
      }
    },
    onConnected: () => socket.send(['EVENT', event.toJson()]),
    onDisconnected: (error) {
      if (!result.isCompleted) {
        result.completeError(error ?? Exception('Relay disconnected'));
      }
    },
  );
  final resultFuture = result.future.timeout(timeout);
  try {
    await socket.connect();
    return await resultFuture;
  } finally {
    await socket.disconnect();
  }
}
