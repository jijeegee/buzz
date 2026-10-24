import 'dart:async';
import 'dart:isolate';

import 'package:sqlite3/sqlite3.dart';

import 'message_db.dart';

/// The one read-write connection to a message database file, owned by a
/// long-lived isolate so no write runs on the UI isolate. Writes apply in the
/// order they were sent, each in its own transaction.
class MessageDbWriter {
  MessageDbWriter._(this.path);

  /// Starts the writer isolate for [path]; writes sent before it is ready
  /// wait for it.
  factory MessageDbWriter.start(String path) =>
      MessageDbWriter._(path).._spawn();

  final String path;
  final _port = ReceivePort();
  final _ready = Completer<SendPort>();
  final _waiting = <int, Completer<void>>{};
  int _nextId = 0;
  bool _closing = false;
  bool _exited = false;

  bool get isClosed => _closing || _exited;

  void _spawn() {
    _port.listen(_onMessage);
    Isolate.spawn(
      _writerMain,
      (_port.sendPort, path),
      onExit: _port.sendPort,
      debugName: 'MessageDbWriter',
    ).catchError((Object error) {
      _fail(error);
      return Isolate.current;
    });
  }

  void _onMessage(Object? message) {
    switch (message) {
      case final SendPort port:
        _ready.complete(port);
      case (final int id, final String? error):
        final completer = _waiting.remove(id);
        if (error == null) {
          completer?.complete();
        } else {
          completer?.completeError(MessageDbWriteException(error));
        }
      case null:
        // The isolate exited (onExit): nothing more will be answered.
        _fail(StateError('Message database writer exited.'));
    }
  }

  void _fail(Object error) {
    _exited = true;
    if (!_ready.isCompleted) _ready.completeError(error);
    for (final completer in _waiting.values) {
      completer.completeError(error);
    }
    _waiting.clear();
    _port.close();
  }

  /// Applies [write] in one transaction.
  Future<void> write(MessageDbWrite write) {
    if (isClosed) {
      return Future.error(StateError('Message database writer is closed.'));
    }
    return _send(write);
  }

  /// Resolves once every write sent so far has been answered.
  Future<void> settled() => Future.wait([
    for (final completer in _waiting.values)
      completer.future.then<void>((_) {}, onError: (Object _) {}),
  ]);

  /// Lets writes already sent finish, then closes the connection. Writes
  /// sent afterwards fail.
  Future<void> close() async {
    if (isClosed) return;
    _closing = true;
    try {
      await _send(const _Close());
    } catch (_) {
      // Already gone.
    }
  }

  Future<void> _send(Object command) {
    final id = _nextId++;
    final completer = Completer<void>();
    _waiting[id] = completer;
    // Every send awaits the same future, so they go out in call order.
    _ready.future.then(
      (port) => port.send((id, command)),
      onError: (Object _) {},
    );
    return completer.future;
  }
}

class MessageDbWriteException implements Exception {
  const MessageDbWriteException(this.message);

  final String message;

  @override
  String toString() => 'MessageDbWriteException: $message';
}

class _Close {
  const _Close();
}

void _writerMain((SendPort, String) args) {
  final (reply, path) = args;
  final commands = ReceivePort();
  Database? db;
  Object? openError;
  try {
    db = openMessageDb(path);
  } catch (error) {
    openError = error;
  }
  reply.send(commands.sendPort);
  commands.listen((message) {
    final (id, command) = message as (int, Object);
    if (command is _Close) {
      db?.close();
      db = null;
      reply.send((id, null));
      commands.close();
      return;
    }
    final open = db;
    if (open == null) {
      reply.send((id, 'Message database unavailable: $openError'));
      return;
    }
    try {
      applyMessageWrite(open, command as MessageDbWrite);
      reply.send((id, null));
    } catch (error) {
      reply.send((id, '$error'));
    }
  });
}
