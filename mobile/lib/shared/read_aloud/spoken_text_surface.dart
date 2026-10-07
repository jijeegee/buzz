import 'dart:math' as math;

import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

/// Paints speech progress over the existing layout without changing text,
/// intercepting gestures, or creating another accessibility reading order.
class SpokenTextSurface extends SingleChildRenderObjectWidget {
  const SpokenTextSurface({
    super.key,
    required super.child,
    required this.spokenText,
    required this.start,
    required this.end,
    required this.color,
  });
  final String? spokenText;
  final int start;
  final int end;
  final Color color;

  @override
  SpokenTextRenderBox createRenderObject(BuildContext context) =>
      SpokenTextRenderBox()..update(spokenText, start, end, color);

  @override
  void updateRenderObject(
    BuildContext context,
    SpokenTextRenderBox renderObject,
  ) => renderObject.update(spokenText, start, end, color);
}

class _TextRun {
  const _TextRun(this.paragraph, this.sourceStart, this.start, this.end);
  final RenderParagraph paragraph;
  final int sourceStart;
  final int start;
  final int end;
}

class _Document {
  final buffer = StringBuffer();
  final runs = <_TextRun>[];
  String get text => buffer.toString();

  void visit(RenderObject object, {bool inline = false}) {
    if (object is RenderOffstage && object.offstage) return;
    if (object is RenderParagraph) {
      // Placeholder children (mentions, links, etc.) are read at their inline
      // position rather than appended after the surrounding paragraph.
      final children = <RenderObject>[];
      object.visitChildren(children.add);
      final text = object.text.toPlainText(includeSemanticsLabels: false);
      // Icon fonts use private-use code points. Exclude their glyphs, while
      // retaining visible labels inside ExcludeSemantics-based mention pills.
      if (text.isNotEmpty &&
          text.runes.every(
            (rune) =>
                (rune >= 0xe000 && rune <= 0xf8ff) ||
                (rune >= 0xf0000 && rune <= 0x10fffd),
          )) {
        return;
      }
      var cursor = 0;
      var childIndex = 0;
      for (var i = 0; i <= text.length; i++) {
        if (i != text.length && text.codeUnitAt(i) != 0xfffc) continue;
        if (i > cursor) {
          final start = buffer.length;
          buffer.write(text.substring(cursor, i));
          runs.add(_TextRun(object, cursor, start, buffer.length));
        }
        if (i < text.length && childIndex < children.length) {
          visit(children[childIndex++], inline: true);
        }
        cursor = i + 1;
      }
      if (!inline) buffer.write('\n');
      return;
    }
    object.visitChildren((child) => visit(child, inline: inline));
  }
}

/// Maps UTF-16 speech ranges onto the laid-out paragraphs, including wrapped
/// lines and inline widgets. Only the active message traverses on progress.
class SpokenTextRenderBox extends RenderProxyBox {
  String? _spokenText;
  int _start = 0;
  int _end = 0;
  Color _color = const Color(0xff4488ff);

  void update(String? text, int start, int end, Color color) {
    if (_spokenText == text &&
        _start == start &&
        _end == end &&
        _color == color) {
      return;
    }
    _spokenText = text;
    _start = start;
    _end = end;
    _color = color;
    markNeedsPaint();
  }

  _Document _document() {
    final document = _Document();
    if (child != null) document.visit(child!);
    return document;
  }

  /// Captures only the text actually laid out in this message body.
  String captureText() => _document().text;

  /// Returns local highlight rectangles; shared by painting and layout tests.
  List<Rect> highlightRects() {
    if (_spokenText == null || _end <= _start) return const [];
    final document = _document();
    // A message edit/profile-label change must not highlight unrelated text.
    if (document.text != _spokenText) return const [];
    return [
      for (final run in document.runs)
        if (run.start < _end && run.end > _start && run.paragraph.attached)
          for (final box in run.paragraph.getBoxesForSelection(
            TextSelection(
              baseOffset: run.sourceStart + math.max(0, _start - run.start),
              extentOffset:
                  run.sourceStart +
                  math.min(run.end - run.start, _end - run.start),
            ),
          ))
            _clippedRect(run.paragraph, box.toRect()),
    ];
  }

  Rect _clippedRect(RenderParagraph paragraph, Rect rect) {
    var result = MatrixUtils.transformRect(
      paragraph.getTransformTo(this),
      rect,
    );
    RenderObject current = paragraph;
    while (!identical(current, this)) {
      final parent = current.parent;
      if (parent == null) break;
      final clip = parent.describeApproximatePaintClip(current);
      if (clip != null) {
        result = result.intersect(
          MatrixUtils.transformRect(parent.getTransformTo(this), clip),
        );
      }
      current = parent;
    }
    return result;
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    super.paint(context, offset);
    for (final rect in highlightRects()) {
      if (rect.isEmpty) continue;
      final shifted = rect.shift(offset);
      context.canvas.drawRect(
        shifted,
        Paint()..color = _color.withValues(alpha: 0.14),
      );
      context.canvas.drawLine(
        shifted.bottomLeft,
        shifted.bottomRight,
        Paint()
          ..color = _color
          ..strokeWidth = 2,
      );
    }
  }
}
