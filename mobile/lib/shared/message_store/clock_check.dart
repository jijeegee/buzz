/// How far this device's clock is from the relay's, from the `Date` header
/// of `/query` answers. Store opens rely on coverage times taken from this
/// clock, so a clock far off (or never compared) opens from the relay
/// (docs/message-sync.md#clock-check). Mirrors desktop
/// `message_store/clock.rs`.
library;

import 'dart:io' show HttpDate, HttpException;

/// A larger difference turns store opens off; it is not corrected.
const maxClockSkewSecs = 60;

int? _skewSecs;

/// Record the relay's `Date` header against [nowSecs].
void noteServerDate(String? header, int nowSecs) {
  if (header == null) return;
  try {
    final relay = HttpDate.parse(header).millisecondsSinceEpoch ~/ 1000;
    _skewSecs = nowSecs - relay;
  } on HttpException {
    // Unreadable: keep the last comparison.
  }
}

/// Whether the last comparison was within [maxClockSkewSecs].
bool get clockTrusted {
  final skew = _skewSecs;
  return skew != null && skew.abs() <= maxClockSkewSecs;
}

/// Forget the comparison (tests).
void resetClockCheck() => _skewSecs = null;
