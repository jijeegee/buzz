import 'dart:math' as math;

import 'package:intl/intl.dart';

import '../../shared/l10n/l10n.dart';
import 'channel.dart';

class EphemeralChannelDisplay {
  final String? detailLabel;
  final String tooltipLabel;

  const EphemeralChannelDisplay({
    required this.detailLabel,
    required this.tooltipLabel,
  });
}

bool isEphemeralChannel(Channel channel) =>
    channel.ttlSeconds != null || channel.ttlDeadline != null;

EphemeralChannelDisplay? ephemeralChannelDisplay(
  Channel channel, {
  DateTime? now,
}) {
  if (!isEphemeralChannel(channel)) return null;

  final deadline = channel.ttlDeadline;
  final remainingSeconds = deadline == null
      ? null
      : ((deadline.millisecondsSinceEpoch -
                    (now ?? DateTime.now()).millisecondsSinceEpoch) /
                1000)
            .ceil();

  if (remainingSeconds == null) {
    final ttlSeconds = channel.ttlSeconds;
    return EphemeralChannelDisplay(
      detailLabel: ttlSeconds == null ? null : formatCompactTtl(ttlSeconds),
      tooltipLabel: ttlSeconds == null
          ? appL10n.ephemeralTooltipNoTtl
          : _ttlTooltip(ttlSeconds),
    );
  }

  return EphemeralChannelDisplay(
    detailLabel: formatCompactRemaining(remainingSeconds),
    tooltipLabel: remainingSeconds <= 0
        ? appL10n.ephemeralTooltipDueNow
        : _remainingTooltip(
            remainingSeconds,
            formatAbsoluteDeadline(deadline!),
          ),
  );
}

enum _TtlUnit { second, minute, hour, day }

/// [seconds] as a count of its largest whole unit, rounded up. Under a
/// minute counts as one minute unless [keepSeconds].
(_TtlUnit, int) _ttlParts(int seconds, {required bool keepSeconds}) {
  if (seconds < 60) {
    return keepSeconds
        ? (_TtlUnit.second, math.max(1, seconds))
        : (_TtlUnit.minute, 1);
  }
  if (seconds < 60 * 60) {
    return (_TtlUnit.minute, math.max(1, (seconds / 60).ceil()));
  }
  if (seconds < 60 * 60 * 24) {
    return (_TtlUnit.hour, math.max(1, (seconds / (60 * 60)).ceil()));
  }
  return (_TtlUnit.day, math.max(1, (seconds / (60 * 60 * 24)).ceil()));
}

String formatCompactRemaining(int remainingSeconds) {
  final l10n = appL10n;
  if (remainingSeconds <= 0) return l10n.ephemeralCleanupDue;
  final (unit, count) = _ttlParts(remainingSeconds, keepSeconds: false);
  return switch (unit) {
    _TtlUnit.second || _TtlUnit.minute => l10n.ephemeralLeftMinutes(count),
    _TtlUnit.hour => l10n.ephemeralLeftHours(count),
    _TtlUnit.day => l10n.ephemeralLeftDays(count),
  };
}

String _remainingTooltip(int remainingSeconds, String deadline) {
  final l10n = appL10n;
  final (unit, count) = _ttlParts(remainingSeconds, keepSeconds: false);
  return switch (unit) {
    _TtlUnit.second ||
    _TtlUnit.minute => l10n.ephemeralTooltipInMinutes(count, deadline),
    _TtlUnit.hour => l10n.ephemeralTooltipInHours(count, deadline),
    _TtlUnit.day => l10n.ephemeralTooltipInDays(count, deadline),
  };
}

String formatCompactTtl(int ttlSeconds) {
  final l10n = appL10n;
  final (unit, count) = _ttlParts(ttlSeconds, keepSeconds: true);
  return switch (unit) {
    _TtlUnit.second => l10n.ephemeralTtlSeconds(count),
    _TtlUnit.minute => l10n.ephemeralTtlMinutes(count),
    _TtlUnit.hour => l10n.ephemeralTtlHours(count),
    _TtlUnit.day => l10n.ephemeralTtlDays(count),
  };
}

String _ttlTooltip(int ttlSeconds) {
  final l10n = appL10n;
  final (unit, count) = _ttlParts(ttlSeconds, keepSeconds: true);
  return switch (unit) {
    _TtlUnit.second => l10n.ephemeralTooltipAfterSeconds(count),
    _TtlUnit.minute => l10n.ephemeralTooltipAfterMinutes(count),
    _TtlUnit.hour => l10n.ephemeralTooltipAfterHours(count),
    _TtlUnit.day => l10n.ephemeralTooltipAfterDays(count),
  };
}

String formatAbsoluteDeadline(DateTime deadline) {
  final local = deadline.toLocal();
  final hour = local.hour.toString().padLeft(2, '0');
  final minute = local.minute.toString().padLeft(2, '0');
  return appL10n.ephemeralDeadline(
    DateFormat.MMMd().format(local),
    '$hour:$minute',
  );
}
