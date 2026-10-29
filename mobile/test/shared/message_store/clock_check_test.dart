import 'dart:io' show HttpDate;

import 'package:flutter_test/flutter_test.dart';

import 'package:kuzz/shared/message_store/clock_check.dart';

void main() {
  test('a clock far from the relay or never compared is not trusted', () {
    resetClockCheck();
    expect(clockTrusted, isFalse);
    const header = 'Thu, 29 Oct 2026 05:13:43 GMT';
    final relay = HttpDate.parse(header).millisecondsSinceEpoch ~/ 1000;
    noteServerDate(header, relay + 60);
    expect(clockTrusted, isTrue);
    noteServerDate('not a date', relay + 600);
    expect(clockTrusted, isTrue);
    noteServerDate(header, relay - 61);
    expect(clockTrusted, isFalse);
  });
}
