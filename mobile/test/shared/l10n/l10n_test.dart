import 'package:kuzz/shared/l10n/l10n.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const hans = Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hans');
  const hant = Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hant');
  const supported = [
    Locale('en'),
    Locale('es'),
    Locale('ko'),
    Locale('pt', 'BR'),
    Locale('fil'),
    hans,
    hant,
  ];

  Locale resolve(List<Locale> preferred) =>
      resolveAppLocale(preferred, supported);

  test('picks the first device language the app ships', () {
    expect(
      resolve(const [Locale('de'), Locale('ko', 'KR')]),
      const Locale('ko'),
    );
    expect(resolve(const [Locale('es', 'MX')]), const Locale('es'));
  });

  test('maps regional Chinese, Portuguese and Tagalog', () {
    expect(resolve(const [Locale('zh', 'TW')]), hant);
    expect(
      resolve(const [
        Locale.fromSubtags(
          languageCode: 'zh',
          scriptCode: 'Hant',
          countryCode: 'HK',
        ),
      ]),
      hant,
    );
    expect(resolve(const [Locale('zh', 'CN')]), hans);
    expect(resolve(const [Locale('zh')]), hans);
    expect(resolve(const [Locale('pt', 'PT')]), const Locale('pt', 'BR'));
    expect(resolve(const [Locale('tl')]), const Locale('fil'));
  });

  test('falls back to English', () {
    expect(resolve(const [Locale('de')]), const Locale('en'));
    expect(resolve(const []), const Locale('en'));
    expect(resolveAppLocale(null, supported), const Locale('en'));
  });

  test('English strings are available without localizations installed', () {
    expect(appL10n.navFriends, 'Friends');
  });
}
