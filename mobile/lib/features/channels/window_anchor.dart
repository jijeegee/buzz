/// Which way a NIP-CW window page runs from its cursor.
///
/// [older] pages run `(created_at DESC, id ASC)` from the cursor, the only
/// direction a newest page has. [newer] pages run the mirrored
/// `(created_at ASC, id DESC)` order, oldest first, and always carry a cursor;
/// relays advertise them as `nip-cw-newer`.
enum WindowPageDirection { older, newer }

/// The NIP-11 `supported_extensions` entry for newer window pages.
const newerWindowExtension = 'nip-cw-newer';

/// The lowest event id: as a cursor id it sorts before every row of its second.
const zeroEventId =
    '0000000000000000000000000000000000000000000000000000000000000000';

/// The cursor a window opens from at an anchor. An older page from it starts
/// at the anchor and a newer page from it holds everything after, so the two
/// pages meet with no gap and no overlap.
///
/// A message anchor uses the id just below [eventId]: in relay order that
/// position sits immediately before the message, so the message itself (with
/// its thread summary and aux closure) leads the older page. Without an id
/// (a read marker), the zero id makes the older page take every row at
/// [createdAt] and the newer page every row after it.
({int createdAt, String eventId}) windowAnchorCursor({
  required int createdAt,
  String? eventId,
}) => (createdAt: createdAt, eventId: _precedingEventId(eventId));

String _precedingEventId(String? eventId) {
  if (eventId == null || eventId.length != 64) return zeroEventId;
  final value = BigInt.tryParse(eventId, radix: 16);
  if (value == null || value == BigInt.zero) return zeroEventId;
  return (value - BigInt.one).toRadixString(16).padLeft(64, '0');
}
