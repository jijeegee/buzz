import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_state/read_state_format.dart';
import '../../shared/read_state/read_state_provider.dart';
import '../../shared/relay/relay.dart';

/// Reads are collected this long before one `mark_read` request.
const markReadBatchDelay = Duration(seconds: 1);

/// Most items per `mark_read` request (relay `MAX_MARK_READ_ITEMS`).
const markReadMaxItems = 100;

/// Most contexts kept waiting; the oldest positions are dropped beyond it.
const _maxPending = 500;

/// Consecutive transient failures before retries wait for the next read.
const _maxRetries = 6;

/// A chat-state ping this soon after one of this device's own writes is that
/// write's echo.
const ownWritePingWindow = Duration(seconds: 3);

final _uuidPattern = RegExp(
  r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$',
  caseSensitive: false,
);
final _eventIdPattern = RegExp(r'^[0-9a-f]{64}$', caseSensitive: false);
final _refusedItemPattern = RegExp(r'mark_read item (\d+):');

/// One `mark_read` item: `{"channel": uuid, "at": secs}` or
/// `{"thread": root, "at": secs}`.
@immutable
class MarkReadItem {
  const MarkReadItem({
    required this.isThread,
    required this.id,
    required this.at,
  });

  final bool isThread;
  final String id;
  final int at;

  String get key => '${isThread ? 'thread' : 'channel'}:$id';

  Map<String, dynamic> toJson() => {
    isThread ? 'thread' : 'channel': id,
    'at': at,
  };
}

/// A read-state context as a `mark_read` item. `msg:` and other schemes have
/// no server position; malformed ids would fail the whole request.
MarkReadItem? markReadItemForContext(String contextId, int unixTimestamp) {
  if (unixTimestamp <= 0) return null;
  if (contextId.startsWith(threadContextPrefix)) {
    final root = contextId.substring(threadContextPrefix.length);
    return _eventIdPattern.hasMatch(root)
        ? MarkReadItem(
            isThread: true,
            id: root.toLowerCase(),
            at: unixTimestamp,
          )
        : null;
  }
  return _uuidPattern.hasMatch(contextId)
      ? MarkReadItem(
          isThread: false,
          id: contextId.toLowerCase(),
          at: unixTimestamp,
        )
      : null;
}

Map<String, dynamic> buildMarkReadFilter(List<MarkReadItem> items) => {
  'mark_read': 1,
  'items': [for (final item in items) item.toJson()],
  'unread_mark': false,
};

/// Index of the item the relay refused (403/404 `mark_read item N: ...`).
int? refusedMarkReadItem(Object error) {
  if (error is! RelayException) return null;
  if (error.statusCode != 403 && error.statusCode != 404) return null;
  final match = _refusedItemPattern.firstMatch(error.body);
  return match == null ? null : int.tryParse(match.group(1)!);
}

bool _isChatStateDisabled(Object error) =>
    error is RelayException &&
    error.statusCode == 404 &&
    error.body.contains('chat_state_disabled');

/// Mirrors this device's reads to the relay's chat state, so other devices'
/// Chats lists see them. Reads merge per context (newest wins), go out in
/// batches after [markReadBatchDelay], and flush when the app hides. A
/// context the relay refuses (not a member) is never resent this session;
/// "chat_state_disabled" stops the writer. Mirrors desktop's
/// `ChatStateWriter` for reads (no unread marks or follows yet).
class ChatStateWriter {
  ChatStateWriter({required this.send, DateTime Function()? now})
    : _now = now ?? DateTime.now;

  final Future<Object?> Function(Map<String, dynamic> filter) send;
  final DateTime Function() _now;

  final _pending = <String, MarkReadItem>{};
  final _sent = <String, int>{};
  final _refused = <String>{};
  Timer? _timer;
  Future<void>? _inFlight;
  int _failures = 0;
  bool _stopped = false;
  DateTime _lastWriteAt = DateTime.fromMillisecondsSinceEpoch(0);

  bool get stopped => _stopped;

  /// Whether a ping arriving now is likely the echo of this device's write.
  bool isOwnWriteEcho() => _now().difference(_lastWriteAt) < ownWritePingWindow;

  void markRead(String contextId, int unixTimestamp) {
    if (_stopped) return;
    final item = markReadItemForContext(contextId, unixTimestamp);
    if (item == null || _refused.contains(item.key)) return;
    if ((_sent[item.key] ?? 0) >= item.at) return;
    final queued = _pending[item.key];
    if (queued != null && queued.at >= item.at) return;
    _pending.remove(item.key);
    _pending[item.key] = item;
    while (_pending.length > _maxPending) {
      _pending.remove(_pending.keys.first);
    }
    _timer ??= Timer(markReadBatchDelay, () {
      _timer = null;
      unawaited(flush());
    });
  }

  /// Sends everything queued now, in chunks of [markReadMaxItems].
  Future<void> flush() async {
    _timer?.cancel();
    _timer = null;
    while (_inFlight != null) {
      await _inFlight;
    }
    if (_stopped || _pending.isEmpty) return;
    final run = _sendAll();
    _inFlight = run;
    try {
      await run;
    } finally {
      _inFlight = null;
    }
  }

  Future<void> _sendAll() async {
    while (!_stopped && _pending.isNotEmpty) {
      final batch = _pending.values.take(markReadMaxItems).toList();
      for (final item in batch) {
        _pending.remove(item.key);
      }
      final ok = await _sendBatch(batch);
      if (!ok) return;
    }
  }

  /// Returns false when sending should pause until the next read.
  Future<bool> _sendBatch(List<MarkReadItem> batch) async {
    var items = batch;
    for (var attempt = 0; attempt < 2 && items.isNotEmpty; attempt++) {
      try {
        _lastWriteAt = _now();
        await send(buildMarkReadFilter(items));
        _lastWriteAt = _now();
        for (final item in items) {
          if ((_sent[item.key] ?? 0) < item.at) _sent[item.key] = item.at;
        }
        _failures = 0;
        return true;
      } catch (error) {
        if (_isChatStateDisabled(error)) {
          stop();
          return false;
        }
        final refused = refusedMarkReadItem(error);
        if (refused != null && refused >= 0 && refused < items.length) {
          _refused.add(items[refused].key);
          items = [...items]..removeAt(refused);
          continue;
        }
        if (error is RelayException &&
            error.statusCode >= 400 &&
            error.statusCode < 500 &&
            error.statusCode != 429) {
          // Malformed for this relay: retrying would fail the same way.
          debugPrint(
            '[ChatStateWriter] mark_read dropped: ${error.statusCode}',
          );
          return true;
        }
        _failures++;
        for (final item in items) {
          final queued = _pending[item.key];
          if (queued == null || queued.at < item.at) _pending[item.key] = item;
        }
        if (_failures < _maxRetries) {
          final backoff = Duration(
            milliseconds: (1000 * (1 << _failures)).clamp(1000, 60000),
          );
          _timer ??= Timer(backoff, () {
            _timer = null;
            unawaited(flush());
          });
        }
        return false;
      }
    }
    if (items.isEmpty) return true;
    // Refused twice in one batch: send the rest in the next batch.
    for (final item in items) {
      final queued = _pending[item.key];
      if (queued == null || queued.at < item.at) _pending[item.key] = item;
    }
    _timer ??= Timer(markReadBatchDelay, () {
      _timer = null;
      unawaited(flush());
    });
    return false;
  }

  void stop() {
    _stopped = true;
    _timer?.cancel();
    _timer = null;
    _pending.clear();
  }
}

/// The writer for the signed-in account. Rebuilt (and the old one stopped)
/// when the account or relay changes, so nothing queued for one account is
/// sent as another. Kept alive by the home screen.
final chatStateWriterProvider = Provider<ChatStateWriter?>((ref) {
  ref.watch(relayConfigProvider);
  final pubkey = ref.watch(myPubkeyProvider);
  if (pubkey == null || pubkey.isEmpty) return null;
  final session = ref.read(relaySessionProvider.notifier);
  final writer = ChatStateWriter(
    send: (filter) => session.queryRelayJson([filter]),
  );
  ref.onDispose(writer.stop);

  // Only reads made on this device, never seeds or other devices' reads
  // (as desktop mirrors only `markContextRead`).
  final reads = ref
      .read(readStateProvider.notifier)
      .localReads
      .listen((read) => writer.markRead(read.contextId, read.unixTimestamp));
  ref.onDispose(reads.cancel);
  ref.listen(appLifecycleProvider, (_, next) {
    if (next == AppLifecycleState.paused ||
        next == AppLifecycleState.hidden ||
        next == AppLifecycleState.detached) {
      unawaited(writer.flush());
    }
  });
  return writer;
});
