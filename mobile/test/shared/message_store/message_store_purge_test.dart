import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/message_store/message_store.dart';

const _relay = 'wss://relay.example';
const _other =
    'bb00000000000000000000000000000000000000000000000000000000000002';

/// test-fixtures/message-store-purge.json, shared with desktop
/// (message_store/tests.rs).
void main() {
  sqfliteFfiInit();
  final fixture =
      jsonDecode(
            File(
              '../test-fixtures/message-store-purge.json',
            ).readAsStringSync(),
          )
          as Map<String, dynamic>;
  final account = fixture['account'] as String;
  final relayKey = fixture['relay_key'] as String;
  final seed = fixture['seed'] as Map<String, dynamic>;

  late Directory dir;
  final stores = <MessageStore>[];

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('message_store_purge');
  });

  tearDown(() async {
    for (final store in stores) {
      await store.forget(_relay, account);
    }
    stores.clear();
    await dir.delete(recursive: true);
  });

  MessageStore open({required bool relayKeyKnown}) {
    final store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
    stores.add(store);
    if (relayKeyKnown) {
      store.noteRelayKey(store.lease(_relay, account), relayKey);
    }
    return store;
  }

  void extend(MessageStore store, Map<String, dynamic> span, int? captured) {
    store.extendCoverage(
      store.lease(_relay, account),
      (span['rooms'] as List).cast<String>(),
      span['from'] as int,
      span['to'] as int,
      captured: captured,
    );
  }

  Future<Map<String, Object?>> state(MessageStore store) async {
    final db = await databaseFactoryFfi.openDatabase(
      await store.pathOf(store.lease(_relay, account)!),
    );
    try {
      Future<List<Object?>> column(String sql) async =>
          (await db.rawQuery(sql)).map((row) => row.values.first).toList();
      final coverage = <String, List<List<int>>>{};
      for (final row in await db.rawQuery(
        'SELECT room, lo_at, hi_at FROM coverage ORDER BY room, lo_at',
      )) {
        (coverage[row['room']! as String] ??= []).add([
          row['lo_at']! as int,
          row['hi_at']! as int,
        ]);
      }
      return {
        'events': await column('SELECT id FROM events ORDER BY id'),
        'targets': await column('SELECT DISTINCT id FROM targets ORDER BY id'),
        'summaries': await column('SELECT root FROM summaries ORDER BY root'),
        'coverage': coverage,
      };
    } finally {
      await db.close();
    }
  }

  for (final raw in fixture['cases'] as List) {
    final testCase = raw as Map<String, dynamic>;
    final name = testCase['name'] as String;
    test(name, () async {
      final known = testCase['relay_key_known'] != false;
      var store = open(relayKeyKnown: known);
      store.submit(store.lease(_relay, account), seed['events'] as List);
      for (final span in seed['coverage'] as List) {
        extend(store, span as Map<String, dynamic>, null);
      }
      if (testCase['seed_list'] != false) {
        final lease = store.lease(_relay, account);
        store.forgetRoomsExcept(
          store.capture(lease),
          lease,
          (seed['list'] as List).cast<String>().toSet(),
        );
      }
      final captures = <String, StoreCapture>{};
      for (final raw in testCase['steps'] as List) {
        final step = raw as Map<String, dynamic>;
        final lease = store.lease(_relay, account);
        StoreCapture? named(Object? name) =>
            name is String ? captures[name] : store.capture(lease);
        if (step['restart'] == true) {
          await store.flushed();
          store = open(relayKeyKnown: known);
          captures.clear();
        } else if (step['capture'] case final String name) {
          captures[name] = store.capture(lease)!;
        } else if (step['events'] case final List events) {
          store.submit(lease, events);
        } else if (step['coverage'] case final Map<String, dynamic> span) {
          extend(store, span, named(span['captured'])?.sequence);
        } else if (step['forget_except'] case final List keep) {
          final capture = step['scope'] == 'stale'
              ? store.capture(store.lease(_relay, _other))
              : named(step['captured']);
          final accepted = store.forgetRoomsExcept(
            capture,
            lease,
            keep.cast<String>().toSet(),
          );
          expect(!accepted, step['refused'], reason: '$name: refused');
        }
        // `forget_except: null`: the list failed and nothing is called.
      }
      await store.flushed();
      expect(await state(store), testCase['expect'], reason: name);
    });
  }

  test('a purge is refused after sign-out', () async {
    final store = open(relayKeyKnown: true);
    final lease = store.lease(_relay, account);
    final capture = store.capture(lease);
    expect(capture, isNotNull);
    await store.forget(_relay, account);
    expect(store.forgetRoomsExcept(capture, lease, {'room-b'}), isFalse);
    expect(store.capture(lease), isNull);
    expect(store.forgetRoomsExcept(null, null, {'room-b'}), isFalse);
  });
}
