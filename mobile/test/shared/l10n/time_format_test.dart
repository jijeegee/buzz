import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:intl/date_symbol_data_local.dart';
import 'package:intl/intl.dart';
import 'package:kuzz/shared/l10n/l10n.dart';
import 'package:kuzz/shared/l10n/time_format.dart';

void main() {
  final march31 = DateTime(2026, 3, 31, 15, 5);
  final now = DateTime(2026, 4, 20);

  tearDown(() => Intl.defaultLocale = null);

  test('English by default', () {
    expect(shortMonthDay(march31, now: now), 'Mar 31');
    expect(shortMonthDay(march31, now: DateTime(2027)), 'Mar 31, 2026');
    expect(recentDayLabel(0, march31), 'Today');
    expect(recentDayLabel(3, march31), 'Tuesday');
    expect(recentDayLabel(9, march31), isNull);
    expect(compactAgo(5 * 60, march31), '5m ago');
    expect(compactAgo(8 * 86400, march31), '3/31/2026');
  });

  test('dates follow the app language', () async {
    await initializeDateFormatting('ko');
    await initializeDateFormatting('de');
    Intl.defaultLocale = intlLocaleFor(const Locale('ko'));
    expect(shortMonthDay(march31, now: now), '3월 31일');
    expect(recentDayLabel(3, march31), '화요일');
    Intl.defaultLocale = intlLocaleFor(const Locale('de'));
    expect(shortMonthDay(march31, now: now), '31. März');
  });

  test('intl locale for each app language', () {
    expect(
      intlLocaleFor(
        const Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hant'),
      ),
      'zh_TW',
    );
    expect(
      intlLocaleFor(
        const Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hans'),
      ),
      'zh_CN',
    );
    expect(intlLocaleFor(const Locale('pt', 'BR')), 'pt_BR');
    expect(intlLocaleFor(const Locale('ko')), 'ko');
  });
}
