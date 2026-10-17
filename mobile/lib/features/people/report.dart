import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';

/// NIP-56 report categories in the order shown, matching desktop's
/// `ReportMessageDialog` and the relay's accepted types. `other` comes last
/// so it reads as the fallback.
enum ReportReason {
  spam('spam', 'Spam'),
  profanity('profanity', 'Profanity or hate speech'),
  nudity('nudity', 'Nudity or sexual content'),
  impersonation('impersonation', 'Impersonation'),
  malware('malware', 'Malware or scam'),
  illegal('illegal', 'Illegal content'),
  other('other', 'Other');

  const ReportReason(this.wireValue, this.label);

  /// The NIP-56 report type on the wire.
  final String wireValue;
  final String label;
}

/// The tags of a kind 1984 report. A person report carries the type on its
/// only `p` tag; a message report names the author in `p` and carries the
/// type on the `e` tag, as desktop's `submitReport` and the relay's
/// `parse_report` expect.
List<List<String>> reportTags({
  required String pubkey,
  required ReportReason reason,
  String? eventId,
}) {
  final author = pubkey.toLowerCase();
  if (eventId == null) {
    return [
      ['p', author, reason.wireValue],
    ];
  }
  return [
    ['p', author],
    ['e', eventId, reason.wireValue],
  ];
}

/// Sends reports to the community's moderators. Reports are never shown to
/// the reported person or anyone else.
class ReportService {
  ReportService(this._ref);

  final Ref _ref;

  /// Reports [pubkey], or with [eventId] one of their messages. Throws when
  /// the relay refuses or does not answer.
  Future<void> submit({
    required String pubkey,
    required ReportReason reason,
    String? eventId,
  }) async {
    final relay = SignedEventRelay.forConfig(
      session: _ref.read(relaySessionProvider.notifier),
      config: _ref.read(relayConfigProvider),
    );
    await relay.submit(
      kind: EventKind.report,
      content: '',
      tags: reportTags(pubkey: pubkey, reason: reason, eventId: eventId),
    );
  }
}

final reportServiceProvider = Provider<ReportService>(ReportService.new);
