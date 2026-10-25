import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/message_db/message_db.dart';
import 'package:kuzz/shared/message_db/message_db_writer.dart';
import 'package:kuzz/shared/message_db/message_store.dart';

import 'message_db_test_support.dart';

/// The store as the app uses it: one writer isolate per file.
void main() {
  late Directory dir;
  late MessageStore store;
  final excluded = <String>[];

  const alice = MessageScope(relayUrl: 'https://one.example', pubkey: 'alice');
  const bob = MessageScope(relayUrl: 'https://one.example', pubkey: 'bob');
  const aliceElsewhere = MessageScope(
    relayUrl: 'https://two.example',
    pubkey: 'alice',
  );

  setUp(() {
    dir = Directory.systemTemp.createTempSync('message_store_test');
    excluded.clear();
    store = MessageStore(
      () async => '${dir.path}/messages',
      onDirectoryCreated: (path) async => excluded.add(path),
    );
  });

  tearDown(() async {
    await store.close();
    dir.deleteSync(recursive: true);
  });

  String pathOf(MessageScope scope) =>
      '${dir.path}/messages/${scope.relativePath}';

  List<String> idsIn(MessageScope scope) {
    if (!File(pathOf(scope)).existsSync()) return const [];
    final reader = openMessageDbReader(pathOf(scope));
    try {
      return storedIds(reader);
    } finally {
      reader.close();
    }
  }

  MessageDbWrite rows(List<String> ids) =>
      MessageDbWrite(rows: rowsOf([for (final id in ids) testEvent(id)]));

  test('writes land in the scope\'s own file, in order', () async {
    await Future.wait([
      store.write(alice, rows(['a1'])),
      store.write(bob, rows(['b1'])),
      store.write(aliceElsewhere, rows(['x1'])),
      store.write(alice, rows(['a2'])),
    ]);

    expect(idsIn(alice), ['a1', 'a2']);
    expect(idsIn(bob), ['b1']);
    expect(idsIn(aliceElsewhere), ['x1']);
    expect(excluded, ['${dir.path}/messages']);
  });

  test('clearing one account keeps the others', () async {
    await store.write(alice, rows(['a1']));
    await store.write(bob, rows(['b1']));
    await store.write(aliceElsewhere, rows(['x1']));

    await store.clear(alice);

    expect(File(pathOf(alice)).existsSync(), isFalse);
    expect(idsIn(bob), ['b1']);
    expect(idsIn(aliceElsewhere), ['x1']);
  });

  test('a clear during a pending write leaves nothing behind', () async {
    final pending = [
      for (var i = 0; i < 20; i++) store.write(alice, rows(['a$i'])),
    ];
    final cleared = store.clear(alice);
    await Future.wait([...pending, cleared]);

    expect(File(pathOf(alice)).existsSync(), isFalse);
    expect(File('${pathOf(alice)}-wal').existsSync(), isFalse);
  });

  test('a write queued before a clear is dropped after it', () async {
    await store.write(alice, rows(['a1']));
    final queuedAt = store.generation;

    await store.clear(alice);
    await store.write(alice, rows(['late']), generation: queuedAt);
    expect(File(pathOf(alice)).existsSync(), isFalse);

    // A write made after the clear starts a fresh file.
    await store.write(alice, rows(['fresh']));
    expect(idsIn(alice), ['fresh']);
  });

  test('clearing another account does not drop this one\'s writes', () async {
    final queuedAt = store.generation;
    await store.clear(bob);

    await store.write(alice, rows(['a1']), generation: queuedAt);

    expect(idsIn(alice), ['a1']);
  });

  test('clearAll removes every file', () async {
    await store.write(alice, rows(['a1']));
    await store.write(aliceElsewhere, rows(['x1']));

    await store.clearAll();

    expect(Directory('${dir.path}/messages').listSync(), isEmpty);
  });

  test(
    'a store reopened on the same directory reads what was written',
    () async {
      await store.write(alice, rows(['a1']));
      await store.close();

      store = MessageStore(() async => '${dir.path}/messages');
      await store.write(alice, rows(['a2']));

      expect(idsIn(alice), ['a1', 'a2']);
    },
  );

  test('without storage, writes are dropped quietly', () async {
    final unavailable = MessageStore(() async => throw StateError('none'));
    addTearDown(unavailable.close);

    await unavailable.write(alice, rows(['a1']));
    await unavailable.clear(alice);
  });

  group('MessageDbWriter', () {
    test('creates the file and applies writes in its isolate', () async {
      final writer = MessageDbWriter.start('${dir.path}/w/one.db');
      addTearDown(writer.close);
      await writer.write(rows(['a']));
      final db = openMessageDbReader('${dir.path}/w/one.db');
      addTearDown(db.close);
      expect(storedIds(db), ['a']);
    });

    test('writes after close fail', () async {
      final writer = MessageDbWriter.start('${dir.path}/w/two.db');
      await writer.write(rows(['a']));
      await writer.close();

      expect(writer.isClosed, isTrue);
      await expectLater(writer.write(rows(['b'])), throwsStateError);
    });

    test('a file that cannot be opened fails each write', () async {
      // A directory where the file should be.
      Directory('${dir.path}/w/three.db').createSync(recursive: true);
      final writer = MessageDbWriter.start('${dir.path}/w/three.db');
      addTearDown(writer.close);

      await expectLater(
        writer.write(rows(['a'])),
        throwsA(isA<MessageDbWriteException>()),
      );
    });
  });
}
