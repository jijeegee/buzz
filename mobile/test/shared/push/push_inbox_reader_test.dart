import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/push/push_inbox.dart';

/// The `origin` and `file` cases of test-fixtures/push-inbox.json.
void main() {
  final fixture =
      jsonDecode(File('../test-fixtures/push-inbox.json').readAsStringSync())
          as Map<String, dynamic>;

  group('origin', () {
    for (final raw in fixture['origin'] as List) {
      final c = raw as Map<String, dynamic>;
      test(c['relay_url'] as String, () {
        expect(pushInboxOrigin(c['relay_url'] as String), c['origin']);
      });
    }
  });

  group('file', () {
    late Directory folder;
    setUp(() async {
      folder = await Directory.systemTemp.createTemp('push-inbox-read');
    });
    tearDown(() => folder.delete(recursive: true));

    for (final raw in fixture['file'] as List) {
      final c = raw as Map<String, dynamic>;
      test(c['name'] as String, () async {
        final name = c['file_name'] as String? ?? '1700000000000-a.json';
        final path = '${folder.path}/$name';
        final body = jsonEncode(c['file']);
        File(path).writeAsStringSync(
          body +
              ' ' *
                  ((c['pad_bytes'] as int? ?? 0) - body.length).clamp(
                    0,
                    1 << 30,
                  ),
        );
        final read = await readPushInboxFolder(
          folder: folder.path,
          relayUrl: fixture['file_relay_url'] as String,
          account: fixture['file_account'] as String,
        );
        bool named(String p) => p.split(RegExp(r'[\\/]')).last == name;
        final accepted = read.files.where((f) => named(f.path)).toList();
        expect(accepted, hasLength(c['accept'] == true ? 1 : 0));
        expect(read.refused.any(named), c['delete'] == true);
        if (accepted.isNotEmpty) {
          final events = (c['file'] as Map)['events'] as List;
          final ids =
              (c['ids'] as List?)?.cast<String>() ??
              [for (final e in events) (e as Map)['id'] as String];
          expect(
            [
              for (final e in [
                ...accepted.single.events,
                ...accepted.single.profiles,
              ])
                e['id'],
            ]..sort(),
            ids.toList()..sort(),
          );
          expect(accepted.single.fetchedAt, (c['file'] as Map)['fetchedAt']);
        }
      });
    }
  });

  test('a missing folder reads as empty', () async {
    final read = await readPushInboxFolder(
      folder: '${Directory.systemTemp.path}/no-such-push-inbox-folder',
      relayUrl: 'wss://relay.example',
      account: 'a' * 64,
    );
    expect(read.files, isEmpty);
    expect(read.refused, isEmpty);
  });

  test('files read oldest first, events ascending, cursor ignored', () async {
    final folder = await Directory.systemTemp.createTemp('push-inbox-order');
    addTearDown(() => folder.delete(recursive: true));
    Map<String, dynamic> event(int n, int at) => {
      'id': n.toRadixString(16).padLeft(64, '0'),
      'pubkey': 'b' * 64,
      'created_at': at,
      'kind': 9,
      'tags': [
        ['h', 'r'],
      ],
      'content': '',
      'sig': '0' * 128,
    };
    String file(int fetchedAt, List<Map<String, dynamic>> events) =>
        jsonEncode({
          'v': 1,
          'relayUrl': 'wss://relay.example',
          'account': 'a' * 64,
          'fetchedAt': fetchedAt,
          'events': events,
        });
    File(
      '${folder.path}/1700000000002-b.json',
    ).writeAsStringSync(file(2, [event(3, 30)]));
    File(
      '${folder.path}/1700000000001-a.json',
    ).writeAsStringSync(file(1, [event(2, 20), event(1, 10), event(1, 10)]));
    File('${folder.path}/1700000000003-c.json').writeAsStringSync(
      file(3, [
        {...event(4, 40), 'sig': 'x'},
        {...event(5, 50), 'created_at': '50'},
        {
          ...event(6, 60),
          'tags': [
            ['h', 1],
          ],
        },
      ]),
    );
    File('${folder.path}/cursor').writeAsStringSync('5');
    final read = await readPushInboxFolder(
      folder: folder.path,
      relayUrl: 'wss://relay.example',
      account: 'A' * 64,
    );
    expect([for (final f in read.files) f.fetchedAt], [1, 2, 3]);
    expect(read.files.last.events, isEmpty, reason: 'shape checked per field');
    expect(
      [for (final e in read.files.first.events) e['created_at']],
      [10, 20],
    );
    expect(read.refused, isEmpty);
  });
}
