import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/push/push_inbox.dart';
import 'package:kuzz/shared/push/push_inbox_writer.dart';

/// The `caps` and `cursor` cases of test-fixtures/push-inbox.json, as the
/// Swift `PushInboxTests` read them, plus the file the writer produces.
void main() {
  final fixture =
      jsonDecode(File('../test-fixtures/push-inbox.json').readAsStringSync())
          as Map<String, dynamic>;
  final token = 'ab' * 16;
  late Directory root;

  setUp(() async {
    root = await Directory.systemTemp.createTemp('push-inbox-writer');
  });
  tearDown(() => root.delete(recursive: true));

  group('caps', () {
    final raw = fixture['caps_limits'] as Map<String, dynamic>;
    final limits = PushInboxLimits(
      maxAgeMilliseconds: raw['max_age_ms'] as int,
      maxEvents: raw['max_events'] as int,
      maxBytes: raw['max_bytes'] as int,
    );
    test('fixture limits are the standard ones', () {
      expect(
        limits.maxAgeMilliseconds,
        PushInboxLimits.standard.maxAgeMilliseconds,
      );
      expect(limits.maxEvents, PushInboxLimits.standard.maxEvents);
      expect(limits.maxBytes, PushInboxLimits.standard.maxBytes);
    });
    for (final item in fixture['caps'] as List) {
      final c = item as Map<String, dynamic>;
      test(c['name'], () async {
        final folder = Directory('${root.path}/$token')..createSync();
        for (final file in c['files'] as List) {
          final f = file as Map<String, dynamic>;
          final text = f.containsKey('raw')
              ? f['raw'] as String
              : jsonEncode({
                  'v': 1,
                  'relayUrl': 'wss://relay.example',
                  'account': 'a' * 64,
                  'fetchedAt': 1,
                  'events': List.generate(f['events'] as int, (i) => {'i': i}),
                  if (f['content_bytes'] != null)
                    'pad': 'x' * (f['content_bytes'] as int),
                });
          File('${folder.path}/${f['name']}').writeAsStringSync(text);
        }
        final writer = PushInboxWriter(
          root.path,
          limits: limits,
          now: () => DateTime.fromMillisecondsSinceEpoch(c['now_ms'] as int),
        );
        await writer.prune(token);
        final kept =
            folder
                .listSync()
                .map((entity) => entity.uri.pathSegments.last)
                .toList()
              ..sort();
        expect(kept, c['kept']);
      });
    }
  });

  group('cursor', () {
    final limits = fixture['cursor_limits'] as Map<String, dynamic>;
    test('fixture limits', () {
      expect(PushInboxCursor.lookbackSeconds, limits['lookback_secs']);
      expect(PushInboxCursor.windowSeconds, limits['window_secs']);
    });
    for (final item in fixture['cursor'] as List) {
      final c = item as Map<String, dynamic>;
      test(c['name'], () {
        if (c.containsKey('since')) {
          expect(
            PushInboxCursor.since(
              cursor: c['cursor'] as int?,
              now: c['now'] as int,
            ),
            c['since'],
          );
        }
        if (c.containsKey('next')) {
          expect(
            PushInboxCursor.next(
              current: c['cursor'] as int?,
              returned: [
                for (final value in c['returned'] as List) value as int,
              ],
              fallback: c['fallback'] == true,
            ),
            c['next'],
          );
        }
      });
    }
  });

  group('write', () {
    final event = {
      'id': '1' * 64,
      'pubkey': 'b' * 64,
      'created_at': 1000,
      'kind': 9,
      'tags': [
        ['h', 'room'],
      ],
      'content': 'hi https://x.example/a',
      'sig': '0' * 128,
      'extra': 'dropped',
    };

    test('writes one whole file in the reader format', () async {
      final writer = PushInboxWriter(
        root.path,
        now: () => DateTime.fromMillisecondsSinceEpoch(1700000000123),
        makeId: () => 'id1',
      );
      final file = await writer.write(
        token: token,
        relayUrl: 'wss://relay.example',
        account: 'a' * 64,
        fetchedAt: 1700000000,
        events: [event],
      );
      expect(file!.path, '${root.path}/$token/1700000000123-id1.json');
      expect(
        Directory(
          '${root.path}/$token',
        ).listSync().map((e) => e.uri.pathSegments.last),
        ['1700000000123-id1.json'],
        reason: 'no .tmp file is left behind',
      );
      final decoded = jsonDecode(file.readAsStringSync()) as Map;
      expect(decoded['v'], 1);
      expect(decoded['relayUrl'], 'wss://relay.example');
      expect(decoded['account'], 'a' * 64);
      expect(decoded['fetchedAt'], 1700000000);
      final written = (decoded['events'] as List).single as Map;
      expect(written.containsKey('extra'), isFalse);
      expect(written['content'], event['content']);
      expect(
        file.readAsStringSync().contains(r'\/'),
        isFalse,
        reason: 'slashes stay unescaped, as Swift writes them',
      );
      // The app's reader takes the file.
      final read = readPushInboxFolderSync(
        folder: '${root.path}/$token',
        relayUrl: 'wss://relay.example',
        account: 'a' * 64,
      );
      expect(read.refused, isEmpty);
      expect(read.files, hasLength(1));
    });

    test('nothing to keep or a bad token writes nothing', () async {
      final writer = PushInboxWriter(root.path);
      expect(
        await writer.write(
          token: token,
          relayUrl: 'wss://relay.example',
          account: 'a' * 64,
          fetchedAt: 1,
          events: const [],
        ),
        isNull,
      );
      expect(
        await writer.write(
          token: '../escape',
          relayUrl: 'wss://relay.example',
          account: 'a' * 64,
          fetchedAt: 1,
          events: [event],
        ),
        isNull,
      );
      expect(root.listSync(), isEmpty);
    });

    test('cursor round trip; damaged cursor reads as none', () async {
      final writer = PushInboxWriter(root.path);
      expect(await writer.readCursor(token), isNull);
      await writer.writeCursor(token, 1234);
      expect(await writer.readCursor(token), 1234);
      File('${root.path}/$token/cursor').writeAsStringSync('x');
      expect(await writer.readCursor(token), isNull);
    });
  });
}
