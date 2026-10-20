import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/group_photo_field.dart';
import 'package:kuzz/features/channels/media_viewer_page.dart';
import 'package:kuzz/shared/relay/media_auth.dart';
import 'package:kuzz/shared/relay/media_image.dart';
import 'package:kuzz/shared/theme/theme.dart';

const _photo =
    'https://relay.example/media/'
    '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.png';

final _channel = Channel(
  id: 'group-1',
  name: 'design',
  channelType: 'stream',
  visibility: 'open',
  description: '',
  createdBy: 'abc',
  createdAt: DateTime(2025),
  memberCount: 3,
  picture: _photo,
);

Future<void> _pump(WidgetTester tester, {String? picture}) => tester.pumpWidget(
  ProviderScope(
    overrides: [
      mediaGetAuthServiceProvider.overrideWithValue(
        MediaGetAuthService(baseUrl: 'https://relay.example', nsec: null),
      ),
      mediaHttpClientProvider.overrideWithValue(
        http_testing.MockClient((_) async => http.Response('', 404)),
      ),
    ],
    child: MaterialApp(
      theme: AppTheme.light(),
      home: Scaffold(
        body: GroupPhotoField(
          channel: _channel,
          picture: picture,
          isSaving: false,
          onChange: () {},
          onRemove: () {},
        ),
      ),
    ),
  ),
);

void main() {
  testWidgets('tapping the group photo opens the original full size', (
    tester,
  ) async {
    await _pump(tester, picture: _photo);
    expect(find.bySemanticsLabel('View group photo'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('group-photo-open')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    final viewer = tester.widget<MediaImageViewerPage>(
      find.byType(MediaImageViewerPage),
    );
    expect(viewer.imageUrl, _photo);
  });

  testWidgets('without a photo the letter is not tappable', (tester) async {
    await _pump(tester);

    expect(find.byKey(const ValueKey('group-photo-open')), findsNothing);
  });
}
