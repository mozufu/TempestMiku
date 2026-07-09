part of 'main.dart';

class _MarkdownMessage extends StatelessWidget {
  const _MarkdownMessage({
    required this.tok,
    required this.accent,
    required this.text,
  });

  final _Tok tok;
  final Color accent;
  final String text;

  @override
  Widget build(BuildContext context) {
    final blocks = _parseMarkdown(text);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (var i = 0; i < blocks.length; i++) ...[
          _MarkdownBlockView(tok: tok, accent: accent, block: blocks[i]),
          if (i != blocks.length - 1) SizedBox(height: blocks[i].spacingAfter),
        ],
      ],
    );
  }
}

class _MarkdownBlockView extends StatelessWidget {
  const _MarkdownBlockView({
    required this.tok,
    required this.accent,
    required this.block,
  });

  final _Tok tok;
  final Color accent;
  final _MarkdownBlock block;

  @override
  Widget build(BuildContext context) {
    switch (block.kind) {
      case _MarkdownBlockKind.heading:
        final level = block.level.clamp(1, 4);
        final size = switch (level) {
          1 => 20.0,
          2 => 17.0,
          3 => 15.5,
          _ => 14.5,
        };
        return Text.rich(
          TextSpan(
            children: _inlineSpans(block.text, tok, accent,
                baseStyle: TextStyle(
                  color: tok.text,
                  fontSize: size,
                  fontWeight: FontWeight.w900,
                  height: 1.34,
                )),
          ),
        );
      case _MarkdownBlockKind.paragraph:
        return Text.rich(
          TextSpan(
            children: _inlineSpans(block.text, tok, accent,
                baseStyle: TextStyle(
                  color: tok.text,
                  fontSize: 14,
                  fontWeight: FontWeight.w400,
                  height: 1.62,
                )),
          ),
        );
      case _MarkdownBlockKind.bullet:
      case _MarkdownBlockKind.ordered:
      case _MarkdownBlockKind.checkbox:
        final marker = block.kind == _MarkdownBlockKind.ordered
            ? '${block.index}.'
            : block.kind == _MarkdownBlockKind.checkbox
                ? (block.checked ? '☑' : '☐')
                : '•';
        return Padding(
          padding: EdgeInsets.only(left: block.indent * 14.0),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: block.kind == _MarkdownBlockKind.ordered ? 30 : 22,
                child: Text(
                  marker,
                  style: TextStyle(
                    color: block.kind == _MarkdownBlockKind.checkbox
                        ? accent
                        : tok.muted,
                    fontSize: 14,
                    fontWeight: FontWeight.w800,
                    height: 1.5,
                  ),
                ),
              ),
              Expanded(
                child: Text.rich(
                  TextSpan(
                    children: _inlineSpans(block.text, tok, accent,
                        baseStyle: TextStyle(
                          color: tok.text,
                          fontSize: 14,
                          fontWeight: FontWeight.w400,
                          height: 1.5,
                        )),
                  ),
                ),
              ),
            ],
          ),
        );
      case _MarkdownBlockKind.quote:
        return Container(
          width: double.infinity,
          padding: const EdgeInsets.fromLTRB(11, 9, 11, 9),
          decoration: BoxDecoration(
            color: tok.surface.withOpacity(0.72),
            border: Border(
              left: BorderSide(color: accent, width: 3),
            ),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Text.rich(
            TextSpan(
              children: _inlineSpans(block.text, tok, accent,
                  baseStyle: TextStyle(
                    color: tok.muted,
                    fontSize: 13.5,
                    fontWeight: FontWeight.w600,
                    height: 1.55,
                    fontStyle: FontStyle.italic,
                  )),
            ),
          ),
        );
      case _MarkdownBlockKind.code:
        return Container(
          width: double.infinity,
          padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
          decoration: BoxDecoration(
            color: tok.bg.withOpacity(0.78),
            border: Border.all(color: tok.border),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Text(
            block.text,
            style: TextStyle(
              color: tok.text,
              fontSize: 12.5,
              height: 1.46,
              fontFamily: 'monospace',
            ),
          ),
        );
      case _MarkdownBlockKind.math:
        return Container(
          width: double.infinity,
          padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
          decoration: BoxDecoration(
            color: tok.surface.withOpacity(0.42),
            borderRadius: BorderRadius.circular(8),
          ),
          child: RaTeXFormula(
            latex: block.text,
            fontSize: 18,
            color: tok.text,
            display: true,
            fallbackStyle: TextStyle(
              color: tok.text,
              fontSize: 13,
              height: 1.45,
              fontFamily: 'monospace',
            ),
          ),
        );
      case _MarkdownBlockKind.rule:
        return Container(
          height: 1,
          color: tok.border.withOpacity(0.72),
        );
      case _MarkdownBlockKind.gap:
        return const SizedBox.shrink();
    }
  }
}

enum _MarkdownBlockKind {
  paragraph,
  heading,
  bullet,
  ordered,
  checkbox,
  quote,
  code,
  math,
  rule,
  gap,
}

class _MarkdownBlock {
  const _MarkdownBlock({
    required this.kind,
    this.text = '',
    this.level = 0,
    this.index = 0,
    this.indent = 0,
    this.checked = false,
  });

  final _MarkdownBlockKind kind;
  final String text;
  final int level;
  final int index;
  final int indent;
  final bool checked;

  double get spacingAfter {
    return switch (kind) {
      _MarkdownBlockKind.heading => level <= 2 ? 12 : 8,
      _MarkdownBlockKind.rule => 12,
      _MarkdownBlockKind.gap => 8,
      _MarkdownBlockKind.code => 10,
      _MarkdownBlockKind.math => 10,
      _MarkdownBlockKind.quote => 9,
      _ => 6,
    };
  }
}

List<_MarkdownBlock> _parseMarkdown(String source) {
  final lines = source.replaceAll('\r\n', '\n').split('\n');
  final blocks = <_MarkdownBlock>[];
  final paragraph = <String>[];
  final code = <String>[];
  final mathBlock = <String>[];
  var inCode = false;
  String? mathStart;
  String? mathEnd;

  void flushParagraph() {
    if (paragraph.isEmpty) return;
    blocks.add(_MarkdownBlock(
      kind: _MarkdownBlockKind.paragraph,
      text: paragraph.join(' '),
    ));
    paragraph.clear();
  }

  void flushCode() {
    blocks.add(_MarkdownBlock(
      kind: _MarkdownBlockKind.code,
      text: code.join('\n').trimRight(),
    ));
    code.clear();
  }

  void flushMath() {
    final latex = _normalizeLatex(mathBlock.join('\n'));
    if (latex.isNotEmpty) {
      blocks.add(_MarkdownBlock(
        kind: _MarkdownBlockKind.math,
        text: latex,
      ));
    }
    mathBlock.clear();
    mathStart = null;
    mathEnd = null;
  }

  for (final raw in lines) {
    final line = raw.trimRight();
    final trimmed = line.trim();
    final activeMathEnd = mathEnd;
    if (activeMathEnd != null) {
      final endIndex = trimmed.indexOf(activeMathEnd);
      if (endIndex >= 0) {
        mathBlock.add(trimmed.substring(0, endIndex));
        flushMath();
        final rest = trimmed.substring(endIndex + activeMathEnd.length).trim();
        if (rest.isNotEmpty) paragraph.add(rest);
      } else {
        mathBlock.add(trimmed);
      }
      continue;
    }
    if (trimmed.startsWith('```')) {
      if (inCode) {
        flushCode();
      } else {
        flushParagraph();
      }
      inCode = !inCode;
      continue;
    }
    if (inCode) {
      code.add(raw);
      continue;
    }
    final mathFence = _mathFenceStart(trimmed);
    if (mathFence != null) {
      flushParagraph();
      final afterStart = trimmed.substring(mathFence.start.length).trim();
      final endIndex = afterStart.indexOf(mathFence.end);
      if (endIndex >= 0) {
        mathBlock.add(afterStart.substring(0, endIndex));
        flushMath();
        final rest =
            afterStart.substring(endIndex + mathFence.end.length).trim();
        if (rest.isNotEmpty) paragraph.add(rest);
      } else {
        mathStart = mathFence.start;
        mathEnd = mathFence.end;
        if (afterStart.isNotEmpty) mathBlock.add(afterStart);
      }
      continue;
    }
    if (trimmed.isEmpty) {
      flushParagraph();
      if (blocks.isNotEmpty && blocks.last.kind != _MarkdownBlockKind.gap) {
        blocks.add(const _MarkdownBlock(kind: _MarkdownBlockKind.gap));
      }
      continue;
    }
    final heading = RegExp(r'^(#{1,4})\s+(.+)$').firstMatch(trimmed);
    if (heading != null) {
      flushParagraph();
      blocks.add(_MarkdownBlock(
        kind: _MarkdownBlockKind.heading,
        level: heading.group(1)!.length,
        text: heading.group(2)!.trim(),
      ));
      continue;
    }
    if (RegExp(r'^[-*_]{3,}$').hasMatch(trimmed)) {
      flushParagraph();
      blocks.add(const _MarkdownBlock(kind: _MarkdownBlockKind.rule));
      continue;
    }
    if (trimmed.startsWith('>')) {
      flushParagraph();
      blocks.add(_MarkdownBlock(
        kind: _MarkdownBlockKind.quote,
        text: trimmed.replaceFirst(RegExp(r'^>\s?'), ''),
      ));
      continue;
    }
    final checkbox =
        RegExp(r'^(\s*)[-*]\s+\[([ xX])\]\s+(.+)$').firstMatch(line);
    if (checkbox != null) {
      flushParagraph();
      blocks.add(_MarkdownBlock(
        kind: _MarkdownBlockKind.checkbox,
        indent: checkbox.group(1)!.length ~/ 2,
        checked: checkbox.group(2)!.trim().isNotEmpty,
        text: checkbox.group(3)!.trim(),
      ));
      continue;
    }
    final bullet = RegExp(r'^(\s*)[-*]\s+(.+)$').firstMatch(line);
    if (bullet != null) {
      flushParagraph();
      blocks.add(_MarkdownBlock(
        kind: _MarkdownBlockKind.bullet,
        indent: bullet.group(1)!.length ~/ 2,
        text: bullet.group(2)!.trim(),
      ));
      continue;
    }
    final ordered = RegExp(r'^(\s*)(\d+)[.)]\s+(.+)$').firstMatch(line);
    if (ordered != null) {
      flushParagraph();
      blocks.add(_MarkdownBlock(
        kind: _MarkdownBlockKind.ordered,
        indent: ordered.group(1)!.length ~/ 2,
        index: int.tryParse(ordered.group(2)!) ?? 1,
        text: ordered.group(3)!.trim(),
      ));
      continue;
    }
    paragraph.add(trimmed);
  }
  if (inCode) flushCode();
  if (mathEnd != null) {
    final unclosed = [
      if (mathStart != null) mathStart!,
      ...mathBlock,
    ].join(' ').trim();
    if (unclosed.isNotEmpty) paragraph.add(unclosed);
    mathBlock.clear();
    mathStart = null;
    mathEnd = null;
  } else if (mathBlock.isNotEmpty) {
    flushMath();
  }
  flushParagraph();
  while (blocks.isNotEmpty && blocks.last.kind == _MarkdownBlockKind.gap) {
    blocks.removeLast();
  }
  return blocks;
}

List<InlineSpan> _inlineSpans(
  String text,
  _Tok tok,
  Color accent, {
  required TextStyle baseStyle,
}) {
  final spans = <InlineSpan>[];
  final plain = StringBuffer();
  var index = 0;

  void flushPlain() {
    if (plain.isEmpty) return;
    spans.add(TextSpan(text: plain.toString(), style: baseStyle));
    plain.clear();
  }

  while (index < text.length) {
    final token = _inlineTokenAt(text, index);
    if (token == null) {
      plain.writeCharCode(text.codeUnitAt(index));
      index++;
      continue;
    }
    flushPlain();
    if (token.kind == _InlineTokenKind.bold) {
      spans.add(TextSpan(
        text: token.content,
        style: baseStyle.copyWith(fontWeight: FontWeight.w900),
      ));
    } else if (token.kind == _InlineTokenKind.code) {
      spans.add(TextSpan(
        text: token.content,
        style: baseStyle.copyWith(
          color: accent,
          fontFamily: 'monospace',
          fontWeight: FontWeight.w700,
          backgroundColor: tok.surface.withOpacity(0.85),
        ),
      ));
    } else {
      spans.add(WidgetSpan(
        alignment: PlaceholderAlignment.middle,
        baseline: TextBaseline.alphabetic,
        child: RaTeXFormula(
          latex: _normalizeLatex(token.content),
          fontSize: (baseStyle.fontSize ?? 14) + 1,
          color: tok.text,
          display: false,
          fallbackStyle: baseStyle.copyWith(
            fontFamily: 'monospace',
            fontWeight: FontWeight.w600,
          ),
        ),
      ));
    }
    index = token.end;
  }
  flushPlain();
  return spans;
}

class _MathFence {
  const _MathFence(this.start, this.end);

  final String start;
  final String end;
}

_MathFence? _mathFenceStart(String text) {
  if (text.startsWith(r'$$')) return const _MathFence(r'$$', r'$$');
  if (text.startsWith(r'\\[')) return const _MathFence(r'\\[', r'\\]');
  if (text.startsWith(r'\[')) return const _MathFence(r'\[', r'\]');
  return null;
}

String _normalizeLatex(String latex) {
  var normalized = latex.trim();
  for (final pair in const [
    [r'\\[', r'\\]'],
    [r'\[', r'\]'],
    [r'$$', r'$$'],
    [r'\\(', r'\\)'],
    [r'\(', r'\)'],
    [r'$', r'$'],
  ]) {
    final start = pair[0];
    final end = pair[1];
    if (normalized.startsWith(start) && normalized.endsWith(end)) {
      normalized = normalized
          .substring(start.length, normalized.length - end.length)
          .trim();
      break;
    }
  }
  return normalized.replaceAllMapped(
    RegExp(r'\\\\(?=[A-Za-z\[\]\(\)])'),
    (_) => '\\',
  );
}

enum _InlineTokenKind { bold, code, math }

class _InlineToken {
  const _InlineToken({
    required this.kind,
    required this.content,
    required this.end,
  });

  final _InlineTokenKind kind;
  final String content;
  final int end;
}

_InlineToken? _inlineTokenAt(String text, int index) {
  if (text.startsWith('**', index)) {
    final end = text.indexOf('**', index + 2);
    if (end > index + 2) {
      return _InlineToken(
        kind: _InlineTokenKind.bold,
        content: text.substring(index + 2, end),
        end: end + 2,
      );
    }
  }
  if (text.startsWith('``', index)) {
    final end = text.indexOf('``', index + 2);
    if (end > index + 2) {
      return _InlineToken(
        kind: _InlineTokenKind.code,
        content: text.substring(index + 2, end),
        end: end + 2,
      );
    }
  }
  if (text.startsWith('`', index)) {
    final end = text.indexOf('`', index + 1);
    if (end > index + 1) {
      return _InlineToken(
        kind: _InlineTokenKind.code,
        content: text.substring(index + 1, end),
        end: end + 1,
      );
    }
  }
  final math = _inlineMathTokenAt(text, index);
  if (math != null) return math;
  return null;
}

_InlineToken? _inlineMathTokenAt(String text, int index) {
  for (final pair in const [
    [r'\\(', r'\\)'],
    [r'\(', r'\)'],
  ]) {
    final start = pair[0];
    final endFence = pair[1];
    if (!text.startsWith(start, index)) continue;
    final end = text.indexOf(endFence, index + start.length);
    if (end > index + start.length) {
      return _InlineToken(
        kind: _InlineTokenKind.math,
        content: text.substring(index + start.length, end),
        end: end + endFence.length,
      );
    }
  }
  if (text.startsWith(r'$', index) && !text.startsWith(r'$$', index)) {
    final end = text.indexOf(r'$', index + 1);
    if (end > index + 1 && !text.substring(index + 1, end).contains('\n')) {
      return _InlineToken(
        kind: _InlineTokenKind.math,
        content: text.substring(index + 1, end),
        end: end + 1,
      );
    }
  }
  return null;
}
