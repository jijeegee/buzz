import 'dart:convert';

import 'package:buzz/features/settings/account_page.dart';
import 'package:buzz/shared/profile/user_cache_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import 'token_settings_harness.dart';

/// The real upload service with the picker+upload replaced.
class _FakeUploadService extends MediaUploadService {
  _FakeUploadService()
    : super(
        baseUrl: tokenOrigin,
        nsec: null,
        pickGalleryImage: () async => null,
        pickGalleryVideo: () async => null,
      );

  int uploads = 0;

  @override
  Future<BlobDescriptor?> pickAndUploadImage() async {
    uploads += 1;
    return BlobDescriptor(
      url: 'https://cdn.test/new.png',
      sha256: 'a' * 64,
      size: 10,
      type: 'image/png',
      uploaded: 0,
    );
  }
}

Future<TokenSettingsHarness> _pumpAccount(
  WidgetTester tester, {
  _FakeUploadService? upload,
}) async {
  final h = TokenSettingsHarness();
  h.server.refreshResponses.add(
    (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
  );
  final service = upload ?? _FakeUploadService();
  addTearDown(service.dispose);
  await h.pump(
    tester,
    const AccountPage(origin: tokenOrigin),
    overrides: [mediaUploadServiceProvider.overrideWithValue(service)],
  );
  return h;
}

Future<void> _save(WidgetTester tester) async {
  await tester.ensureVisible(find.byKey(const Key('account-save')));
  await tester.tap(find.byKey(const Key('account-save')));
  await frames(tester);
}

void main() {
  testWidgets('profile save sends one PATCH with the edited fields', (
    tester,
  ) async {
    final h = await _pumpAccount(tester);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('account-display-name')))
          .controller
          ?.text,
      'Ada',
    );

    await tester.enterText(
      find.byKey(const Key('account-display-name')),
      'Ada Lovelace',
    );
    await tester.enterText(find.byKey(const Key('account-username')), 'ada_l');
    await _save(tester);

    final patch = h.account.calls('PATCH', '/auth/profile').single;
    expect(jsonDecode(patch.body), {
      'display_name': 'Ada Lovelace',
      'avatar_url': null,
      'username': 'ada_l',
    });
    expect(patch.headers['Authorization'], 'Bearer bzs_new');
    expect(find.text('Profile saved'), findsOneWidget);
  });

  testWidgets('a new photo is uploaded and saved as avatar_url', (
    tester,
  ) async {
    final upload = _FakeUploadService();
    final h = await _pumpAccount(tester, upload: upload);

    await tester.tap(find.byKey(const Key('account-avatar-change')));
    await frames(tester);
    expect(upload.uploads, 1);
    // Uploading alone persists nothing.
    expect(h.account.calls('PATCH', '/auth/profile'), isEmpty);

    await _save(tester);
    expect(
      jsonDecode(h.account.calls('PATCH', '/auth/profile').single.body),
      containsPair('avatar_url', 'https://cdn.test/new.png'),
    );
  });

  testWidgets('removing the photo saves avatar_url null', (tester) async {
    final h = TokenSettingsHarness();
    h.account.profile['avatar_url'] = 'https://cdn.test/old.png';
    h.server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );
    final service = _FakeUploadService();
    addTearDown(service.dispose);
    await h.pump(
      tester,
      const AccountPage(origin: tokenOrigin),
      overrides: [mediaUploadServiceProvider.overrideWithValue(service)],
    );

    await tester.tap(find.byKey(const Key('account-avatar-remove')));
    await frames(tester);
    await _save(tester);

    expect(
      jsonDecode(h.account.calls('PATCH', '/auth/profile').single.body),
      containsPair('avatar_url', null),
    );
  });

  testWidgets('a taken username is shown and the edits are kept', (
    tester,
  ) async {
    final h = await _pumpAccount(tester);
    h.account.failures['/auth/profile'] = (_) => jsonResponse({
      'error': 'username already taken',
      'code': 'username_taken',
    }, status: 409);

    await tester.enterText(find.byKey(const Key('account-username')), 'bob');
    await _save(tester);

    expect(find.byKey(const Key('account-error')), findsOneWidget);
    expect(find.textContaining('already taken'), findsOneWidget);
    expect(find.text('Profile saved'), findsNothing);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('account-username')))
          .controller
          ?.text,
      'bob',
    );
  });

  testWidgets('an invalid name is rejected before any request', (tester) async {
    final h = await _pumpAccount(tester);

    await tester.enterText(find.byKey(const Key('account-display-name')), ' ');
    await tester.enterText(find.byKey(const Key('account-username')), 'No!');
    await _save(tester);

    expect(h.account.calls('PATCH', '/auth/profile'), isEmpty);
    expect(find.byKey(const Key('account-error')), findsOneWidget);
  });

  testWidgets('a failed load offers a retry', (tester) async {
    final h = TokenSettingsHarness();
    h.server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );
    h.account.failures['/auth/me'] = (_) =>
        jsonResponse({'error': 'unavailable'}, status: 503);
    final service = _FakeUploadService();
    addTearDown(service.dispose);
    await h.pump(
      tester,
      const AccountPage(origin: tokenOrigin),
      overrides: [mediaUploadServiceProvider.overrideWithValue(service)],
    );

    expect(find.textContaining('unavailable'), findsOneWidget);
    await tester.tap(find.byKey(const Key('account-load-retry')));
    await frames(tester);
    expect(find.byKey(const Key('account-display-name')), findsOneWidget);
  });

  testWidgets('a name is measured in characters, not UTF-16 units', (
    tester,
  ) async {
    final h = await _pumpAccount(tester);
    // 40 emoji: 40 characters, 80 UTF-16 code units.
    final name = '\u{1F41D}' * 40;

    await tester.enterText(find.byKey(const Key('account-display-name')), name);
    await _save(tester);

    expect(
      jsonDecode(h.account.calls('PATCH', '/auth/profile').single.body),
      containsPair('display_name', name),
    );
  });

  testWidgets('a saved profile updates the cached self profile', (
    tester,
  ) async {
    final h = await _pumpAccount(tester);

    await tester.enterText(
      find.byKey(const Key('account-display-name')),
      'Ada Lovelace',
    );
    await _save(tester);

    final principal = h.account.profile['principal_id']! as String;
    final cached = h.container.read(userCacheProvider)[principal];
    expect(cached?.displayName, 'Ada Lovelace');
  });
}
