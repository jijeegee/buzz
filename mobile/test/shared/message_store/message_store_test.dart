import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_capture.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';

void main() {
  sqfliteFfiInit();
  final fixture =
      jsonDecode(File('../test-fixtures/message-store.json').readAsStringSync())
          as Map<String, dynamic>;
  final events = (fixture['events'] as List)
      .map((e) => NostrEvent.fromJson(e as Map<String, dynamic>))
      .toList();
  late Directory dir;
  late String path;
  late MessageStore store;
  setUp(() async {
    dir = await Directory.systemTemp.createTemp('kuzz-message-store-');
    path = '${dir.path}/messages.db';
    store = MessageStore(() async => path, factory: databaseFactoryFfi);
  });
  tearDown(() async {
    await store.close();
    await dir.delete(recursive: true);
  });

  test(
    'shared contract retains originals across restart without claiming coverage',
    () async {
      expect(
        messageStoreKinds,
        unorderedEquals(fixture['storedKinds'] as List),
      );
      for (final kind in fixture['excludedKinds'] as List) {
        expect(messageStoreKinds.contains(kind), false);
      }
      var lease = store.begin('https://one', 'a');
      await store.put(lease, events);
      await store.put(lease, events);
      await store.close();
      store = MessageStore(() async => path, factory: databaseFactoryFfi);
      lease = store.begin('https://one', 'a');
      for (final e in events) {
        expect((await store.get(lease, e.id))?.toJson(), e.toJson());
      }
      final db = await databaseFactoryFfi.openDatabase(path);
      expect(
        (await db.rawQuery(
          'SELECT needs_repair FROM scopes',
        )).single['needs_repair'],
        1,
      );
      expect(
        (await db.rawQuery('SELECT COUNT(*) AS n FROM events')).single['n'],
        events.length,
      );
    },
  );

  test(
    'logout fences queued capture and preserves other relay/account',
    () async {
      final a = store.begin('https://one', 'a'),
          b = store.begin('https://one', 'b'),
          c = store.begin('https://two', 'a');
      for (final lease in [a, b, c]) {
        await store.put(lease, events);
      }
      final capture = MessageStoreCapture(store, a);
      capture.receive(events);
      await store.clear('https://one', 'a');
      await capture.flush();
      final fresh = store.begin('https://one', 'a');
      expect(await store.get(fresh, 'root'), null);
      expect(await store.get(b, 'root'), isNotNull);
      expect(await store.get(c, 'root'), isNotNull);
      capture.close();
    },
  );

  test('a clear during first open cannot be undone by a late write', () async {
    await store.close();
    final gate = Completer<String>();
    store = MessageStore(() => gate.future, factory: databaseFactoryFfi);
    final old = store.begin('https://one', 'a');
    final write = store.put(old, events);
    await Future<void>.delayed(Duration.zero);
    final clear = store.clear('https://one', 'a');
    final duringClear = store.begin('https://one', 'a');
    final racingWrite = store.put(duringClear, events);
    gate.complete(path);
    await write;
    await clear;
    await racingWrite;
    await store.put(duringClear, events);
    expect(await store.get(store.begin('https://one', 'a'), 'root'), null);
  });

  test(
    'retains more than the old head limit and captures disk failure',
    () async {
      final lease = store.begin('https://one', 'a');
      for (var batch = 0; batch < 8; batch++) {
        await store.put(
          lease,
          List.generate(
            256,
            (i) => NostrEvent(
              id: '$batch-$i',
              pubkey: 'a',
              createdAt: 1,
              kind: 9,
              tags: [],
              content: 'body',
            ),
          ),
        );
      }
      expect(await store.get(lease, '0-0'), isNotNull);
      expect(await store.get(lease, '7-255'), isNotNull);
      final errors = <Object>[];
      final broken = MessageStore(
        () async => '$path/not-a-directory/db',
        factory: databaseFactoryFfi,
      );
      final capture = MessageStoreCapture(
        broken,
        broken.begin('https://one', 'a'),
        onError: errors.add,
      );
      capture.receive(events);
      await capture.flush();
      expect(errors, isNotEmpty);
      capture.close();
      await broken.close();
    },
  );
}
