import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../auth/account/account_api.dart';
import '../theme/theme_provider.dart';
import 'l10n.dart';

/// Where the chosen app language is saved on this device. Same key as
/// desktop's `kuzz.locale`; absent means "follow the phone".
const _localePreferenceKey = 'kuzz.locale';

/// The shipped language whose tag (`ko`, `zh-Hans`, `pt-BR`) is [tag], or
/// `null` when this build has no such language.
Locale? shippedLocaleForTag(String? tag) {
  if (tag == null) return null;
  for (final locale in shippedLocales) {
    if (locale.toLanguageTag() == tag) return locale;
  }
  return null;
}

/// The language picked in Settings, or `null` to follow the phone. A saved
/// language this build no longer ships also follows the phone.
class LocalePreferenceNotifier extends Notifier<Locale?> {
  @override
  Locale? build() => shippedLocaleForTag(
    ref.watch(savedPrefsProvider).getString(_localePreferenceKey),
  );

  /// Pick [locale], or `null` to follow the phone again.
  Future<void> select(Locale? locale) async {
    state = locale;
    final prefs = ref.read(savedPrefsProvider);
    if (locale == null) {
      await prefs.remove(_localePreferenceKey);
    } else {
      await prefs.setString(_localePreferenceKey, locale.toLanguageTag());
    }
  }
}

final localePreferenceProvider =
    NotifierProvider<LocalePreferenceNotifier, Locale?>(
      LocalePreferenceNotifier.new,
    );

/// Saves the language the app is showing on the signed-in account, so text
/// the server writes (such as moderation notices) uses it. Place it inside
/// `MaterialApp.builder`, below the localizations.
class AccountLocaleSync extends ConsumerStatefulWidget {
  const AccountLocaleSync({super.key, required this.child});

  final Widget child;

  @override
  ConsumerState<AccountLocaleSync> createState() => _AccountLocaleSyncState();
}

class _AccountLocaleSyncState extends ConsumerState<AccountLocaleSync> {
  String? _synced;

  @override
  Widget build(BuildContext context) {
    final origin = ref.watch(accountOriginProvider);
    final locale = Localizations.maybeLocaleOf(context);
    if (origin != null && locale != null) {
      final tag = locale.toLanguageTag();
      final key = '$origin $tag';
      if (_synced != key) {
        _synced = key;
        final api = ref.read(accountApiProvider(origin));
        unawaited(_save(api, tag));
      }
    }
    return widget.child;
  }

  static Future<void> _save(AccountApi api, String tag) async {
    try {
      final me = await api.me();
      if (me.locale != tag) await api.setLocale(tag);
    } catch (_) {
      // Offline or signed out: tried again the next time the app starts or
      // the language changes.
    }
  }
}
