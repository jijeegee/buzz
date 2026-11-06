import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/agents/external_agent_profile_sheet.dart';
import 'package:kuzz/l10n/app_localizations.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:mocktail/mocktail.dart';

const _origin = 'https://relay.test';
final _agent = 'a' * 64;

class _Api extends Mock implements AccountApi {}

/// The relay's answer arrives when [answer] completes.
class _Cache extends UserCacheNotifier {
  _Cache(this.held, this.answer);

  final Map<String, UserProfile> held;
  final Future<bool> Function() answer;
  final refreshed = <String>[];

  @override
  Map<String, UserProfile> build() => held;

  @override
  Future<bool> refresh(List<String> pubkeys) {
    refreshed.addAll(pubkeys);
    return answer();
  }
}

Future<void> _open(WidgetTester tester, _Api api, _Cache cache) async {
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        accountOriginProvider.overrideWithValue(_origin),
        accountApiProvider.overrideWith((ref, _) => api),
        userCacheProvider.overrideWith(() => cache),
      ],
      child: MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => showExternalAgentProfileSheet(
              context,
              agentPubkey: _agent,
              name: 'Hermes',
            ),
            child: const Text('open'),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('open'));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 400));
}

void main() {
  void stubSave(_Api api) {
    when(
      () => api.setExternalAgentProfile(
        any(),
        displayName: any(named: 'displayName'),
        avatarUrl: any(named: 'avatarUrl'),
        about: any(named: 'about'),
      ),
    ).thenAnswer((_) async {});
  }

  testWidgets('the form opens only on the relay answer and keeps what it '
      'did not change', (tester) async {
    final api = _Api();
    stubSave(api);
    final answer = Completer<bool>();
    final cache = _Cache({
      _agent: UserProfile(
        pubkey: _agent,
        displayName: 'Sales',
        avatarUrl: 'https://example.com/a.png',
        about: 'Sells socks',
      ),
    }, () => answer.future);
    await _open(tester, api, cache);

    expect(find.byKey(const Key('agent-profile-save')), findsNothing);
    answer.complete(true);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await tester.enterText(
      find.byKey(const Key('agent-profile-name')),
      ' Support ',
    );
    await tester.pump();
    await tester.ensureVisible(find.byKey(const Key('agent-profile-save')));
    await tester.tap(find.byKey(const Key('agent-profile-save')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));

    verify(
      () => api.setExternalAgentProfile(
        _agent,
        displayName: 'Support',
        avatarUrl: 'https://example.com/a.png',
        about: 'Sells socks',
      ),
    ).called(1);
    expect(find.byKey(const Key('agent-profile-name')), findsNothing);
  });

  testWidgets('a failed read opens no form, so nothing can be cleared', (
    tester,
  ) async {
    final api = _Api();
    final cache = _Cache(const {}, () async => false);
    await _open(tester, api, cache);
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.byKey(const Key('agent-profile-unavailable')), findsOneWidget);
    expect(find.byKey(const Key('agent-profile-save')), findsNothing);
  });

  testWidgets('a blank name cannot be saved', (tester) async {
    final api = _Api();
    final cache = _Cache(const {}, () async => true);
    await _open(tester, api, cache);
    await tester.pump(const Duration(milliseconds: 100));

    await tester.enterText(find.byKey(const Key('agent-profile-name')), '   ');
    await tester.pump();
    final save = tester.widget<FilledButton>(
      find.byKey(const Key('agent-profile-save')),
    );
    expect(save.onPressed, isNull);
  });
}
