import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// Features import only `shared/` (AGENTS.md mobile rules): the community
/// switcher reaches the sign-in flow through a shared named route.
void main() {
  test('the channels page does not import the sign-in feature', () {
    final files = [
      File('lib/features/channels/channels_page.dart'),
      ...Directory('lib/features/channels/channels_page')
          .listSync()
          .whereType<File>()
          .where((file) => file.path.endsWith('.dart')),
    ];
    final offenders = [
      for (final file in files)
        if (file.readAsStringSync().contains('sign_in/')) file.path,
    ];
    expect(offenders, isEmpty);
  });
}
