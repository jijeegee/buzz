import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'l10n/l10n.dart';

Future<void> copyToClipboard(
  BuildContext context,
  String text, {
  String? message,
}) async {
  final shown = message ?? context.l10n.clipboardCopied;
  await Clipboard.setData(ClipboardData(text: text));
  if (!context.mounted) return;
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(content: Text(shown), duration: const Duration(seconds: 2)),
  );
}
