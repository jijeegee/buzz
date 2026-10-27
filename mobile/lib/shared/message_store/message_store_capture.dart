import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:path_provider/path_provider.dart';

import '../relay/nostr_models.dart';
import 'message_store.dart';

/// One application store; switching communities does not destroy its contents.
/// Widget suites install an in-memory SQLite factory, keeping production writes
/// exercised without platform channels or real filesystem I/O in fakeAsync.
@visibleForTesting
MessageStore Function()? messageStoreFactoryForTests;

final messageStoreProvider = Provider<MessageStore>((ref) {
  final store =
      messageStoreFactoryForTests?.call() ??
      MessageStore(() async {
        final dir = await getApplicationSupportDirectory();
        await Directory(dir.path).create(recursive: true);
        return '${dir.path}/messages.db';
      });
  ref.onDispose(() => unawaited(store.close()));
  return store;
});

/// Observable storage failure, separate from transport/UI delivery. All stored
/// scopes remain needs_repair=1 until the later catch-up stage proves coverage.
final messageStoreFailure = ValueNotifier<Object?>(null);

/// Bounded asynchronous disk writer. UI delivery is never gated on disk I/O.
class MessageStoreCapture {
  MessageStoreCapture(this.store, this.lease, {void Function(Object)? onError})
    : _onError = onError ?? reportMessageStoreFailure;

  final MessageStore store;
  final MessageStoreLease lease;
  final void Function(Object) _onError;
  final List<NostrEvent> _pending = [];
  int _bytes = 0;
  Future<void>? _draining;
  bool _closed = false;

  void receive(Iterable<NostrEvent> events) {
    if (_closed) return;
    for (final event in events) {
      if (!messageStoreKinds.contains(event.kind)) continue;
      final bytes =
          event.content.length * 2 + event.tags.toString().length * 2 + 512;
      if (_pending.length >= 4096 || _bytes + bytes > 8 * 1024 * 1024) {
        _onError(
          StateError('Message store queue full; history repair required'),
        );
        break;
      }
      _pending.add(event);
      _bytes += bytes;
    }
    if (_pending.isNotEmpty && _draining == null) {
      _draining = Future<void>.microtask(_drain);
    }
  }

  Future<void> _drain() async {
    try {
      while (_pending.isNotEmpty) {
        final count = _pending.length.clamp(0, 256);
        final batch = _pending.sublist(0, count);
        _pending.removeRange(0, count);
        // Conservatively count the in-flight batch until it has completed.
        try {
          await store.put(lease, batch);
        } catch (error) {
          _onError(error);
        }
        _bytes = _pending.fold(
          0,
          (sum, event) =>
              sum +
              event.content.length * 2 +
              event.tags.toString().length * 2 +
              512,
        );
      }
    } finally {
      _draining = null;
    }
  }

  Future<void> flush() async {
    await _draining;
  }

  /// A switch drains already received events; explicit logout revokes the lease
  /// through MessageStore.clear before any such completion can write again.
  void close() {
    _closed = true;
    unawaited(flush().whenComplete(() => store.retire(lease)));
  }
}

void reportMessageStoreFailure(Object error) {
  messageStoreFailure.value = error;
  debugPrint('[MessageStore] persistence incomplete: $error');
}
