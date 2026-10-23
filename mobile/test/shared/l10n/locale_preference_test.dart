import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:kuzz/features/settings/language_picker_sheet.dart';
import 'package:kuzz/shared/l10n/l10n.dart';
import 'package:kuzz/shared/l10n/locale_preference.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';

void main() {
  Future<ProviderContainer> container(Map<String, Object> saved) async {
    SharedPreferences.setMockInitialValues(saved);
    final prefs = await SharedPreferences.getInstance();
    final container = ProviderContainer(
      overrides: [savedPrefsProvider.overrideWithValue(prefs)],
    );
    addTearDown(container.dispose);
    return container;
  }

  test('follows the phone until a language is picked', () async {
    final c = await container({});
    expect(c.read(localePreferenceProvider), isNull);

    await c.read(localePreferenceProvider.notifier).select(const Locale('en'));
    expect(c.read(localePreferenceProvider), const Locale('en'));
    final prefs = c.read(savedPrefsProvider);
    expect(prefs.getString('kuzz.locale'), 'en');

    await c.read(localePreferenceProvider.notifier).select(null);
    expect(c.read(localePreferenceProvider), isNull);
    expect(prefs.getString('kuzz.locale'), isNull);
  });

  test('a saved language this build does not ship follows the phone', () async {
    final c = await container({'kuzz.locale': 'xx-Unknown'});
    expect(c.read(localePreferenceProvider), isNull);
    expect(shippedLocaleForTag('en'), const Locale('en'));
  });

  test('languages are listed by their own names', () {
    expect(languageNativeName(sourceLocale), 'English');
    expect(
      lookupAppLocalizations(sourceLocale).settingsLanguageDevice('English'),
      'Same as phone (English)',
    );
  });
}
