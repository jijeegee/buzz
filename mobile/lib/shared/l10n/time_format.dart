import 'package:intl/intl.dart';

import 'l10n.dart';

// Shared date words and formats. Formats are built per call so they follow
// the current app language (`Intl.defaultLocale`, set by `AppL10nScope`), and
// use skeletons so each language orders the parts its own way.

/// "Today", "Yesterday", or the weekday name for 2–6 days ago; null for
/// anything older or in the future. [dayDiff] counts calendar days back.
String? recentDayLabel(int dayDiff, DateTime date) {
  if (dayDiff == 0) return appL10n.timeToday;
  if (dayDiff == 1) return appL10n.timeYesterday;
  if (dayDiff > 1 && dayDiff < 7) return DateFormat.EEEE().format(date);
  return null;
}

/// "Mar 31", or "Mar 31, 2025" when [date] is not in [now]'s year.
String shortMonthDay(DateTime date, {DateTime? now}) {
  final current = now ?? DateTime.now();
  return date.year == current.year
      ? DateFormat.MMMd().format(date)
      : DateFormat.yMMMd().format(date);
}

/// Clock time in the language's usual style ("3:05 PM", "오후 3:05").
String clockTime(DateTime date) => DateFormat.jm().format(date);

/// "just now", "5m ago", "3h ago", "2d ago", then a short numeric date
/// ("3/31/2025" in English) once [time] is a week or more old.
String compactAgo(int ageSeconds, DateTime time) {
  final l10n = appL10n;
  if (ageSeconds < 60) return l10n.timeJustNow;
  if (ageSeconds < 3600) return l10n.timeMinutesAgoShort(ageSeconds ~/ 60);
  if (ageSeconds < 86400) return l10n.timeHoursAgoShort(ageSeconds ~/ 3600);
  if (ageSeconds < 604800) return l10n.timeDaysAgoShort(ageSeconds ~/ 86400);
  return DateFormat.yMd().format(time);
}
