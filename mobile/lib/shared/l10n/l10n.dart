import 'package:flutter/widgets.dart';
import 'package:intl/intl.dart';

import '../../l10n/app_localizations.dart';

export '../../l10n/app_localizations.dart';

/// English: the source language and the fallback for missing strings.
const sourceLocale = Locale('en');

// Regional and legacy tags that map onto one of our language codes.
const _aliases = <String, Locale>{
  'zh_cn': Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hans'),
  'zh_sg': Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hans'),
  'zh_my': Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hans'),
  'zh_tw': Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hant'),
  'zh_hk': Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hant'),
  'zh_mo': Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hant'),
  'zh': Locale.fromSubtags(languageCode: 'zh', scriptCode: 'Hans'),
  'pt': Locale('pt', 'BR'),
  'tl': Locale('fil'),
  'in': Locale('id'),
};

String _tag(Locale locale) => [
  locale.languageCode,
  if (locale.scriptCode != null) locale.scriptCode!,
  if (locale.countryCode != null && locale.countryCode!.isNotEmpty)
    locale.countryCode!,
].join('_').toLowerCase();

/// Picks the first of the device's languages (most preferred first) that the
/// app ships, else English. `zh_Hant_TW` → `zh_Hant`, `es_MX` → `es`,
/// `pt_PT` → `pt_BR`. Same rules as desktop `matchLocale`.
Locale resolveAppLocale(
  List<Locale>? preferred, [
  Iterable<Locale> supported = AppLocalizations.supportedLocales,
]) {
  final byTag = {for (final locale in supported) _tag(locale): locale};
  for (final locale in preferred ?? const <Locale>[]) {
    final parts = _tag(locale).split('_');
    final candidates = [parts.join('_'), parts.take(2).join('_'), parts.first];
    for (final candidate in candidates) {
      final match =
          byTag[candidate] ??
          (_aliases[candidate] == null
              ? null
              : byTag[_tag(_aliases[candidate]!)]);
      if (match != null) return match;
    }
  }
  return sourceLocale;
}

/// The `intl` locale for dates and numbers in [locale]. `intl` has no
/// script-only Chinese data, so Traditional uses Taiwan's and Simplified
/// China's.
String intlLocaleFor(Locale locale) {
  if (locale.languageCode == 'zh') {
    return locale.scriptCode == 'Hant' ? 'zh_TW' : 'zh_CN';
  }
  return Intl.canonicalizedLocale(locale.toString());
}

AppLocalizations? _current;

/// The app-wide strings for code without a [BuildContext] (services,
/// notifications). Widgets use `context.l10n` so text follows language changes.
AppLocalizations get appL10n =>
    _current ?? lookupAppLocalizations(sourceLocale);

extension AppLocalizationsContext on BuildContext {
  /// UI strings in the current language. Falls back to English when no
  /// localizations are installed above (bare widget tests).
  AppLocalizations get l10n =>
      Localizations.of<AppLocalizations>(this, AppLocalizations) ??
      lookupAppLocalizations(sourceLocale);
}

/// Keeps [appL10n] and `Intl.defaultLocale` in step with the language the
/// app is showing. Place it inside `MaterialApp.builder`.
class AppL10nScope extends StatelessWidget {
  const AppL10nScope({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    _current = context.l10n;
    final locale = Localizations.maybeLocaleOf(context);
    // DateFormat / NumberFormat without an explicit locale use the app
    // language, not the device's.
    if (locale != null) Intl.defaultLocale = intlLocaleFor(locale);
    return child;
  }
}
