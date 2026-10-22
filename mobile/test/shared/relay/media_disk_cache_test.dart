import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:kuzz/shared/relay/media_auth.dart';
import 'package:kuzz/shared/relay/media_disk_cache.dart';
import 'package:kuzz/shared/relay/media_image.dart';

// Minimal valid 1x1 transparent PNG.
final _pngBytes = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAA'
  'AAYAAjCB0C8AAAAASUVORK5CYII=',
);

const _relayBase = 'https://relay.example.com';
final _sha = 'a' * 64;
final _thumbUrl = '$_relayBase/media/$_sha.thumb.jpg';
const _scopeA = '$_relayBase\npubkey-a';
const _scopeB = '$_relayBase\npubkey-b';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late Directory root;
  late Directory dir;

  setUp(() async {
    root = await Directory.systemTemp.createTemp('media_disk_cache_test');
    dir = Directory('${root.path}/media');
    MediaImageProvider.debugResetCooldowns();
    PaintingBinding.instance.imageCache.clear();
    PaintingBinding.instance.imageCache.clearLiveImages();
  });

  tearDown(() async {
    // A background write may still hold a file open on Windows.
    for (var attempt = 0; ; attempt++) {
      try {
        await root.delete(recursive: true);
        return;
      } on FileSystemException {
        if (attempt == 20) rethrow;
        await Future<void>.delayed(const Duration(milliseconds: 25));
      }
    }
  });

  List<File> files() =>
      dir.existsSync() ? dir.listSync().whereType<File>().toList() : <File>[];

  group('MediaDiskCache', () {
    test('stores only content-addressed relay media', () {
      expect(MediaDiskCache.isCacheable(_thumbUrl), isTrue);
      expect(
        MediaDiskCache.isCacheable('$_relayBase/media/$_sha.small.jpg'),
        isTrue,
      );
      expect(MediaDiskCache.isCacheable('$_relayBase/media/$_sha'), isTrue);
      expect(
        MediaDiskCache.isCacheable('https://cdn.example.com/avatar.png'),
        isFalse,
      );
      expect(MediaDiskCache.isCacheable('$_thumbUrl?v=2'), isFalse);
      expect(
        MediaDiskCache.isCacheable('$_relayBase/media/abc123.png'),
        isFalse,
      );
    });

    test('reads back a write only in the same scope', () async {
      final cache = MediaDiskCache(Future.value(dir));
      await cache.write(_scopeA, _thumbUrl, _pngBytes);

      expect(await cache.read(_scopeA, _thumbUrl), _pngBytes);
      expect(await cache.read(_scopeB, _thumbUrl), isNull);
      expect(files(), hasLength(1));
      expect(files().single.path, isNot(contains(_sha)));
    });

    test('evicts the least recently used files over the budget', () async {
      final cache = MediaDiskCache(Future.value(dir), maxBytes: 250);
      final chunk = Uint8List(100);
      final urls = [
        for (final c in ['1', '2', '3']) '$_relayBase/media/${c * 64}.jpg',
      ];
      final base = DateTime(2020, 1, 1);
      for (var i = 0; i < urls.length; i++) {
        final before = {for (final f in files()) f.path};
        await cache.write(_scopeA, urls[i], chunk);
        await cache.evictionIdle;
        // Distinct ages regardless of the file system's clock resolution.
        for (final file in files().where((f) => !before.contains(f.path))) {
          await file.setLastModified(base.add(Duration(minutes: i)));
        }
      }
      expect(files(), hasLength(2));

      expect(await cache.read(_scopeA, urls[0]), isNull);
      expect(await cache.read(_scopeA, urls[1]), isNotNull);
      expect(await cache.read(_scopeA, urls[2]), isNotNull);
    });
  });

  group('MediaImageProvider with a disk cache', () {
    MediaGetAuthService auth(String? scope) =>
        MediaGetAuthService(baseUrl: _relayBase, nsec: null, cacheScope: scope);

    test('a disk hit skips the network', () async {
      final cache = MediaDiskCache(Future.value(dir));
      await cache.write(_scopeA, _thumbUrl, _pngBytes);
      var fetches = 0;
      final client = http_testing.MockClient((request) async {
        fetches += 1;
        return http.Response('nope', 500);
      });
      Uint8List? observed;

      await _wait(
        MediaImageProvider(
          url: _thumbUrl,
          auth: auth(_scopeA),
          client: client,
          diskCache: cache,
          onBytesLoaded: (bytes) => observed = bytes,
        ).resolve(ImageConfiguration.empty),
      );

      expect(fetches, 0);
      expect(observed, _pngBytes);
    });

    test('a miss fetches and writes the bytes', () async {
      final cache = MediaDiskCache(Future.value(dir));
      var fetches = 0;
      final client = http_testing.MockClient((request) async {
        fetches += 1;
        return http.Response.bytes(_pngBytes, 200);
      });

      await _wait(
        MediaImageProvider(
          url: _thumbUrl,
          auth: auth(_scopeA),
          client: client,
          diskCache: cache,
        ).resolve(ImageConfiguration.empty),
      );

      expect(fetches, 1);
      expect(
        await _eventually(() => cache.read(_scopeA, _thumbUrl)),
        _pngBytes,
      );
    });

    test('another account does not read the stored bytes', () async {
      final cache = MediaDiskCache(Future.value(dir));
      await cache.write(_scopeA, _thumbUrl, _pngBytes);
      var fetches = 0;
      final client = http_testing.MockClient((request) async {
        fetches += 1;
        return http.Response.bytes(_pngBytes, 200);
      });

      await _wait(
        MediaImageProvider(
          url: _thumbUrl,
          auth: auth(_scopeB),
          client: client,
          diskCache: cache,
        ).resolve(ImageConfiguration.empty),
      );

      expect(fetches, 1);
      // Its own copy is stored under its own scope.
      expect(
        await _eventually(() => cache.read(_scopeB, _thumbUrl)),
        _pngBytes,
      );
    });

    test('nothing is stored when signed out', () async {
      final cache = MediaDiskCache(Future.value(dir));
      final client = http_testing.MockClient(
        (request) async => http.Response.bytes(_pngBytes, 200),
      );

      await _wait(
        MediaImageProvider(
          url: _thumbUrl,
          auth: auth(null),
          client: client,
          diskCache: cache,
        ).resolve(ImageConfiguration.empty),
      );
      await Future<void>.delayed(const Duration(milliseconds: 50));

      expect(files(), isEmpty);
    });

    test('an unreadable entry is replaced from the network', () async {
      final cache = MediaDiskCache(Future.value(dir));
      await cache.write(_scopeA, _thumbUrl, Uint8List.fromList([1, 2, 3]));
      var fetches = 0;
      final client = http_testing.MockClient((request) async {
        fetches += 1;
        return http.Response.bytes(_pngBytes, 200);
      });

      await _wait(
        MediaImageProvider(
          url: _thumbUrl,
          auth: auth(_scopeA),
          client: client,
          diskCache: cache,
        ).resolve(ImageConfiguration.empty),
      );

      expect(fetches, 1);
      expect(
        await _eventually(() => cache.read(_scopeA, _thumbUrl)),
        _pngBytes,
      );
    });
  });
}

/// Polls [read] until it returns PNG bytes; the provider writes in the
/// background after decoding.
Future<Uint8List?> _eventually(Future<Uint8List?> Function() read) async {
  Uint8List? bytes;
  for (var i = 0; i < 50; i++) {
    bytes = await read();
    if (bytes != null && bytes.length == _pngBytes.length) return bytes;
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
  return bytes;
}

Future<void> _wait(ImageStream stream) {
  final done = Completer<void>();
  late final ImageStreamListener listener;
  listener = ImageStreamListener(
    (image, sync) {
      image.dispose();
      stream.removeListener(listener);
      if (!done.isCompleted) done.complete();
    },
    onError: (error, stack) {
      stream.removeListener(listener);
      if (!done.isCompleted) done.completeError(error, stack);
    },
  );
  stream.addListener(listener);
  return done.future;
}
