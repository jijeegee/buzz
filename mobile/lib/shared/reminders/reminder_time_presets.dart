/// Reminder time presets shared by the "Remind me later" sheet.
///
/// Mirrors `desktop/src/features/reminders/lib/timePresets.ts` — same labels,
/// same "always strictly in the future" guarantee.
library;

import '../l10n/l10n.dart';

class ReminderTimePreset {
  /// The option's text in the given language.
  final String Function(AppLocalizations l10n) labelFor;
  final int Function() getTimestamp;

  const ReminderTimePreset({
    required this.labelFor,
    required this.getTimestamp,
  });

  /// The option's text in the app language.
  String get label => labelFor(appL10n);
}

int _nowSeconds() => DateTime.now().millisecondsSinceEpoch ~/ 1000;

/// Next occurrence of [dayOffset] days from now at 9am local time. If that
/// instant is already past, roll to the following day so the result is
/// always in the future.
int nextDayAt9am(int dayOffset, {DateTime? now}) {
  final current = now ?? DateTime.now();
  var target = DateTime(
    current.year,
    current.month,
    current.day + dayOffset,
    9,
  );
  if (!target.isAfter(current)) {
    target = DateTime(target.year, target.month, target.day + 1, 9);
  }
  return target.millisecondsSinceEpoch ~/ 1000;
}

/// Days until next Monday (1-7), matching the desktop preset semantics.
int daysUntilNextMonday(DateTime now) {
  final days = (DateTime.monday - now.weekday) % 7;
  return days == 0 ? 7 : days;
}

final List<ReminderTimePreset> reminderTimePresets = [
  ReminderTimePreset(
    labelFor: (l10n) => l10n.reminderPresetIn30Minutes,
    getTimestamp: () => _nowSeconds() + 30 * 60,
  ),
  ReminderTimePreset(
    labelFor: (l10n) => l10n.reminderPresetIn1Hour,
    getTimestamp: () => _nowSeconds() + 60 * 60,
  ),
  ReminderTimePreset(
    labelFor: (l10n) => l10n.reminderPresetIn3Hours,
    getTimestamp: () => _nowSeconds() + 3 * 60 * 60,
  ),
  ReminderTimePreset(
    labelFor: (l10n) => l10n.reminderPresetTomorrow,
    getTimestamp: () => nextDayAt9am(1),
  ),
  ReminderTimePreset(
    labelFor: (l10n) => l10n.reminderPresetNextMonday,
    getTimestamp: () => nextDayAt9am(daysUntilNextMonday(DateTime.now())),
  ),
];

/// Combine a picked date and time into a future Unix timestamp (seconds), or
/// null when the instant is not strictly in the future — the shared guard for
/// the custom picker so a past time never fires immediately.
int? combineCustomDateTime(DateTime date, int hour, int minute) {
  final target = DateTime(date.year, date.month, date.day, hour, minute);
  final timestamp = target.millisecondsSinceEpoch ~/ 1000;
  if (timestamp <= _nowSeconds()) return null;
  return timestamp;
}
