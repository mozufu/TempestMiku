part of 'main.dart';

enum _MarkdownBlockKind {
  paragraph,
  heading,
  bullet,
  ordered,
  checkbox,
  quote,
  code,
  table,
  math,
  rule,
  gap,
}

enum _MarkdownTableAlignment { left, center, right }

class _MarkdownBlock {
  const _MarkdownBlock({
    required this.kind,
    this.text = '',
    this.info = '',
    this.level = 0,
    this.index = 0,
    this.indent = 0,
    this.checked = false,
    this.tableRows = const [],
    this.tableAlignments = const [],
  });

  final _MarkdownBlockKind kind;
  final String text;
  final String info;
  final int level;
  final int index;
  final int indent;
  final bool checked;
  final List<List<String>> tableRows;
  final List<_MarkdownTableAlignment> tableAlignments;

  double get spacingAfter {
    return switch (kind) {
      _MarkdownBlockKind.heading => level <= 2 ? 12 : 8,
      _MarkdownBlockKind.rule => 12,
      _MarkdownBlockKind.gap => 8,
      _MarkdownBlockKind.code => 10,
      _MarkdownBlockKind.table => 10,
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
  var codeInfo = '';
  String? mathStart;
  String? mathEnd;

  void flushParagraph() {
    if (paragraph.isEmpty) return;
    blocks.add(
      _MarkdownBlock(
        kind: _MarkdownBlockKind.paragraph,
        text: paragraph.join(' '),
      ),
    );
    paragraph.clear();
  }

  void flushCode() {
    blocks.add(
      _MarkdownBlock(
        kind: _MarkdownBlockKind.code,
        text: code.join('\n').trimRight(),
        info: codeInfo,
      ),
    );
    code.clear();
    codeInfo = '';
  }

  void flushMath() {
    final latex = _normalizeLatex(mathBlock.join('\n'));
    if (latex.isNotEmpty) {
      blocks.add(_MarkdownBlock(kind: _MarkdownBlockKind.math, text: latex));
    }
    mathBlock.clear();
    mathStart = null;
    mathEnd = null;
  }

  for (var lineIndex = 0; lineIndex < lines.length; lineIndex++) {
    final raw = lines[lineIndex];
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
        codeInfo = trimmed.substring(3).trim();
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
    final table = _parseMarkdownTableAt(lines, lineIndex);
    if (table != null) {
      flushParagraph();
      blocks.add(table.block);
      lineIndex = table.lastLineIndex;
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
      blocks.add(
        _MarkdownBlock(
          kind: _MarkdownBlockKind.heading,
          level: heading.group(1)!.length,
          text: heading.group(2)!.trim(),
        ),
      );
      continue;
    }
    if (RegExp(r'^[-*_]{3,}$').hasMatch(trimmed)) {
      flushParagraph();
      blocks.add(const _MarkdownBlock(kind: _MarkdownBlockKind.rule));
      continue;
    }
    if (trimmed.startsWith('>')) {
      flushParagraph();
      blocks.add(
        _MarkdownBlock(
          kind: _MarkdownBlockKind.quote,
          text: trimmed.replaceFirst(RegExp(r'^>\s?'), ''),
        ),
      );
      continue;
    }
    final checkbox = RegExp(
      r'^(\s*)[-*]\s+\[([ xX])\]\s+(.+)$',
    ).firstMatch(line);
    if (checkbox != null) {
      flushParagraph();
      blocks.add(
        _MarkdownBlock(
          kind: _MarkdownBlockKind.checkbox,
          indent: checkbox.group(1)!.length ~/ 2,
          checked: checkbox.group(2)!.trim().isNotEmpty,
          text: checkbox.group(3)!.trim(),
        ),
      );
      continue;
    }
    final bullet = RegExp(r'^(\s*)[-*]\s+(.+)$').firstMatch(line);
    if (bullet != null) {
      flushParagraph();
      blocks.add(
        _MarkdownBlock(
          kind: _MarkdownBlockKind.bullet,
          indent: bullet.group(1)!.length ~/ 2,
          text: bullet.group(2)!.trim(),
        ),
      );
      continue;
    }
    final ordered = RegExp(r'^(\s*)(\d+)[.)]\s+(.+)$').firstMatch(line);
    if (ordered != null) {
      flushParagraph();
      blocks.add(
        _MarkdownBlock(
          kind: _MarkdownBlockKind.ordered,
          indent: ordered.group(1)!.length ~/ 2,
          index: int.tryParse(ordered.group(2)!) ?? 1,
          text: ordered.group(3)!.trim(),
        ),
      );
      continue;
    }
    paragraph.add(trimmed);
  }
  if (inCode) flushCode();
  if (mathEnd != null) {
    final unclosed =
        [if (mathStart != null) mathStart!, ...mathBlock].join(' ').trim();
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

class _ParsedMarkdownTable {
  const _ParsedMarkdownTable({
    required this.block,
    required this.lastLineIndex,
  });

  final _MarkdownBlock block;
  final int lastLineIndex;
}

_ParsedMarkdownTable? _parseMarkdownTableAt(
  List<String> lines,
  int startIndex,
) {
  if (startIndex + 1 >= lines.length || !lines[startIndex].contains('|')) {
    return null;
  }
  final headers = _splitMarkdownTableRow(lines[startIndex]);
  final delimiters = _splitMarkdownTableRow(lines[startIndex + 1]);
  if (headers.length < 2 || headers.length != delimiters.length) return null;
  final delimiterPattern = RegExp(r'^:?-{3,}:?$');
  if (!delimiters.every(delimiterPattern.hasMatch)) return null;

  final alignments = delimiters
      .map((delimiter) {
        if (delimiter.startsWith(':') && delimiter.endsWith(':')) {
          return _MarkdownTableAlignment.center;
        }
        if (delimiter.endsWith(':')) return _MarkdownTableAlignment.right;
        return _MarkdownTableAlignment.left;
      })
      .toList(growable: false);
  final rows = <List<String>>[headers];
  var cursor = startIndex + 2;
  while (cursor < lines.length) {
    final line = lines[cursor];
    if (line.trim().isEmpty || !line.contains('|')) break;
    final cells = _splitMarkdownTableRow(line);
    if (cells.length < 2) break;
    rows.add(
      List<String>.generate(
        headers.length,
        (index) => index < cells.length ? cells[index] : '',
        growable: false,
      ),
    );
    cursor++;
  }
  return _ParsedMarkdownTable(
    block: _MarkdownBlock(
      kind: _MarkdownBlockKind.table,
      tableRows: rows,
      tableAlignments: alignments,
    ),
    lastLineIndex: cursor - 1,
  );
}

List<String> _splitMarkdownTableRow(String line) {
  var content = line.trim();
  if (content.startsWith('|')) content = content.substring(1);
  if (content.endsWith('|') && !content.endsWith(r'\|')) {
    content = content.substring(0, content.length - 1);
  }

  final cells = <String>[];
  final cell = StringBuffer();
  var inCode = false;
  for (var index = 0; index < content.length; index++) {
    final character = content[index];
    if (character == '\\' &&
        index + 1 < content.length &&
        content[index + 1] == '|') {
      cell.write('|');
      index++;
      continue;
    }
    if (character == '`') inCode = !inCode;
    if (character == '|' && !inCode) {
      cells.add(cell.toString().trim());
      cell.clear();
      continue;
    }
    cell.write(character);
  }
  cells.add(cell.toString().trim());
  return cells;
}
