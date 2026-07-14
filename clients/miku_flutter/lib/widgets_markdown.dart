part of 'main.dart';

/// A readable, selectable Markdown surface for Miku-authored content.
///
/// HTTP(S) links are deliberately copy-only. Opening a link remains an
/// explicit action owned by the surrounding client instead of a side effect of
/// rendering untrusted model output.
class MikuMarkdownBody extends StatelessWidget {
  const MikuMarkdownBody({super.key, required this.text, this.accent});

  final String text;
  final Color? accent;

  @override
  Widget build(BuildContext context) {
    final tok = MikuTokens.of(context);
    return _MarkdownMessage(tok: tok, accent: accent ?? tok.accent, text: text);
  }
}

class _MarkdownMessage extends StatelessWidget {
  const _MarkdownMessage({
    required this.tok,
    required this.accent,
    required this.text,
  });

  final MikuTokens tok;
  final Color accent;
  final String text;

  @override
  Widget build(BuildContext context) {
    final blocks = _parseMarkdown(text);
    return SelectionArea(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (var i = 0; i < blocks.length; i++) ...[
            _MarkdownBlockView(tok: tok, accent: accent, block: blocks[i]),
            if (i != blocks.length - 1)
              SizedBox(height: blocks[i].spacingAfter),
          ],
        ],
      ),
    );
  }
}

class _MarkdownBlockView extends StatelessWidget {
  const _MarkdownBlockView({
    required this.tok,
    required this.accent,
    required this.block,
  });

  final MikuTokens tok;
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
            children: _inlineSpans(
              block.text,
              tok,
              accent,
              baseStyle: TextStyle(
                color: tok.text,
                fontSize: size,
                fontWeight: FontWeight.w900,
                height: 1.34,
              ),
            ),
          ),
        );
      case _MarkdownBlockKind.paragraph:
        return Text.rich(
          TextSpan(
            children: _inlineSpans(
              block.text,
              tok,
              accent,
              baseStyle: TextStyle(
                color: tok.text,
                fontSize: 14,
                fontWeight: FontWeight.w400,
                height: 1.62,
              ),
            ),
          ),
        );
      case _MarkdownBlockKind.bullet:
      case _MarkdownBlockKind.ordered:
      case _MarkdownBlockKind.checkbox:
        final marker =
            block.kind == _MarkdownBlockKind.ordered
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
                    color:
                        block.kind == _MarkdownBlockKind.checkbox
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
                    children: _inlineSpans(
                      block.text,
                      tok,
                      accent,
                      baseStyle: TextStyle(
                        color: tok.text,
                        fontSize: 14,
                        fontWeight: FontWeight.w400,
                        height: 1.5,
                      ),
                    ),
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
            color: tok.surface.withValues(alpha: 0.72),
            border: Border(left: BorderSide(color: accent, width: 3)),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Text.rich(
            TextSpan(
              children: _inlineSpans(
                block.text,
                tok,
                accent,
                baseStyle: TextStyle(
                  color: tok.muted,
                  fontSize: 13.5,
                  fontWeight: FontWeight.w600,
                  height: 1.55,
                  fontStyle: FontStyle.italic,
                ),
              ),
            ),
          ),
        );
      case _MarkdownBlockKind.code:
        return Container(
          width: double.infinity,
          decoration: BoxDecoration(
            color: tok.bg.withValues(alpha: 0.78),
            border: Border.all(color: tok.border),
            borderRadius: BorderRadius.circular(8),
          ),
          clipBehavior: Clip.antiAlias,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  const SizedBox(width: 12),
                  Expanded(
                    child: Semantics(
                      header: true,
                      child: Text(
                        block.info.isEmpty ? 'Code' : block.info,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 11.5,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.25,
                        ),
                      ),
                    ),
                  ),
                  _MarkdownCopyButton(
                    key: ValueKey(('markdown-code-copy', block.text)),
                    value: block.text,
                    semanticLabel: 'Copy code',
                    tooltip: 'Copy code',
                    copiedMessage: 'Code copied',
                    color: tok.muted,
                  ),
                ],
              ),
              Divider(height: 1, color: tok.border),
              Padding(
                padding: const EdgeInsets.fromLTRB(11, 10, 11, 12),
                child: _MarkdownHorizontalScroll(
                  semanticsLabel: 'Horizontally scrollable code block',
                  child: Text(
                    block.text,
                    softWrap: false,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 12.5,
                      height: 1.46,
                      fontFamily: 'monospace',
                    ),
                  ),
                ),
              ),
            ],
          ),
        );
      case _MarkdownBlockKind.table:
        return _MarkdownTableView(tok: tok, accent: accent, block: block);
      case _MarkdownBlockKind.math:
        return Container(
          width: double.infinity,
          padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
          decoration: BoxDecoration(
            color: tok.surface.withValues(alpha: 0.42),
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
        return Container(height: 1, color: tok.border.withValues(alpha: 0.72));
      case _MarkdownBlockKind.gap:
        return const SizedBox.shrink();
    }
  }
}

class _MarkdownCopyButton extends StatelessWidget {
  const _MarkdownCopyButton({
    super.key,
    required this.value,
    required this.semanticLabel,
    required this.tooltip,
    required this.copiedMessage,
    required this.color,
    this.icon = Icons.content_copy_outlined,
  });

  final String value;
  final String semanticLabel;
  final String tooltip;
  final String copiedMessage;
  final Color color;
  final IconData icon;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: semanticLabel,
      excludeSemantics: true,
      child: IconButton(
        tooltip: tooltip,
        onPressed: () async {
          await Clipboard.setData(ClipboardData(text: value));
          if (!context.mounted) return;
          final messenger = ScaffoldMessenger.maybeOf(context);
          messenger?.hideCurrentSnackBar();
          messenger?.showSnackBar(
            SnackBar(
              content: Text(copiedMessage),
              duration: const Duration(seconds: 1),
            ),
          );
        },
        constraints: const BoxConstraints.tightFor(width: 48, height: 48),
        padding: const EdgeInsets.all(12),
        iconSize: 18,
        color: color,
        icon: Icon(icon),
      ),
    );
  }
}

class _MarkdownHorizontalScroll extends StatefulWidget {
  const _MarkdownHorizontalScroll({
    required this.semanticsLabel,
    required this.child,
  });

  final String semanticsLabel;
  final Widget child;

  @override
  State<_MarkdownHorizontalScroll> createState() =>
      _MarkdownHorizontalScrollState();
}

class _MarkdownHorizontalScrollState extends State<_MarkdownHorizontalScroll> {
  late final ScrollController _controller;

  @override
  void initState() {
    super.initState();
    _controller = ScrollController();
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Semantics(
      label: widget.semanticsLabel,
      child: Scrollbar(
        controller: _controller,
        child: SingleChildScrollView(
          controller: _controller,
          scrollDirection: Axis.horizontal,
          child: widget.child,
        ),
      ),
    );
  }
}

class _MarkdownTableView extends StatelessWidget {
  const _MarkdownTableView({
    required this.tok,
    required this.accent,
    required this.block,
  });

  final MikuTokens tok;
  final Color accent;
  final _MarkdownBlock block;

  @override
  Widget build(BuildContext context) {
    if (block.tableRows.isEmpty) return const SizedBox.shrink();
    return Container(
      width: double.infinity,
      decoration: BoxDecoration(
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(7),
        child: _MarkdownHorizontalScroll(
          semanticsLabel: 'Horizontally scrollable table',
          child: Table(
            defaultColumnWidth: const IntrinsicColumnWidth(),
            defaultVerticalAlignment: TableCellVerticalAlignment.middle,
            border: TableBorder.symmetric(
              inside: BorderSide(color: tok.border),
            ),
            children: [
              for (
                var rowIndex = 0;
                rowIndex < block.tableRows.length;
                rowIndex++
              )
                TableRow(
                  decoration:
                      rowIndex == 0
                          ? BoxDecoration(
                            color: tok.surface.withValues(alpha: 0.88),
                          )
                          : null,
                  children: [
                    for (
                      var columnIndex = 0;
                      columnIndex < block.tableRows[rowIndex].length;
                      columnIndex++
                    )
                      _MarkdownTableCell(
                        tok: tok,
                        accent: accent,
                        text: block.tableRows[rowIndex][columnIndex],
                        alignment: block.tableAlignments[columnIndex],
                        isHeader: rowIndex == 0,
                      ),
                  ],
                ),
            ],
          ),
        ),
      ),
    );
  }
}

class _MarkdownTableCell extends StatelessWidget {
  const _MarkdownTableCell({
    required this.tok,
    required this.accent,
    required this.text,
    required this.alignment,
    required this.isHeader,
  });

  final MikuTokens tok;
  final Color accent;
  final String text;
  final _MarkdownTableAlignment alignment;
  final bool isHeader;

  @override
  Widget build(BuildContext context) {
    final style = TextStyle(
      color: tok.text,
      fontSize: 13,
      fontWeight: isHeader ? FontWeight.w800 : FontWeight.w400,
      height: 1.45,
    );
    final content = Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      child: Text.rich(
        TextSpan(children: _inlineSpans(text, tok, accent, baseStyle: style)),
        textAlign: switch (alignment) {
          _MarkdownTableAlignment.left => TextAlign.left,
          _MarkdownTableAlignment.center => TextAlign.center,
          _MarkdownTableAlignment.right => TextAlign.right,
        },
      ),
    );
    return isHeader ? Semantics(header: true, child: content) : content;
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

List<InlineSpan> _inlineSpans(
  String text,
  MikuTokens tok,
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
    if (token.kind == _InlineTokenKind.link) {
      final target = token.target!;
      spans.add(
        TextSpan(
          text: token.content,
          semanticsLabel: '${token.content}, HTTP link to $target',
          style: baseStyle.copyWith(
            color: accent,
            fontWeight: FontWeight.w700,
            decoration: TextDecoration.underline,
            decorationColor: accent,
            decorationThickness: 1.5,
          ),
        ),
      );
      spans.add(
        WidgetSpan(
          alignment: PlaceholderAlignment.middle,
          child: _MarkdownCopyButton(
            key: ValueKey(('markdown-link-copy', target, index)),
            value: target,
            semanticLabel: 'Copy link $target',
            tooltip: 'Copy link',
            copiedMessage: 'Link copied',
            color: accent,
            icon: Icons.link,
          ),
        ),
      );
    } else if (token.kind == _InlineTokenKind.bold) {
      spans.add(
        TextSpan(
          text: token.content,
          style: baseStyle.copyWith(fontWeight: FontWeight.w900),
        ),
      );
    } else if (token.kind == _InlineTokenKind.code) {
      spans.add(
        TextSpan(
          text: token.content,
          style: baseStyle.copyWith(
            color: accent,
            fontFamily: 'monospace',
            fontWeight: FontWeight.w700,
            backgroundColor: tok.surface.withValues(alpha: 0.85),
          ),
        ),
      );
    } else {
      spans.add(
        WidgetSpan(
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
        ),
      );
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
      normalized =
          normalized
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

enum _InlineTokenKind { bold, code, math, link }

class _InlineToken {
  const _InlineToken({
    required this.kind,
    required this.content,
    required this.end,
    this.target,
  });

  final _InlineTokenKind kind;
  final String content;
  final int end;
  final String? target;
}

_InlineToken? _inlineTokenAt(String text, int index) {
  final link = _inlineLinkTokenAt(text, index);
  if (link != null) return link;
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

_InlineToken? _inlineLinkTokenAt(String text, int index) {
  final remaining = text.substring(index);
  final markdownLink = RegExp(
    r'^\[([^\]\n]+)\]\((https?://[^\s)]+)\)',
    caseSensitive: false,
  ).firstMatch(remaining);
  if (markdownLink != null && _isHttpUrl(markdownLink.group(2)!)) {
    return _InlineToken(
      kind: _InlineTokenKind.link,
      content: markdownLink.group(1)!,
      target: markdownLink.group(2)!,
      end: index + markdownLink.end,
    );
  }

  final autolink = RegExp(
    r'^<(https?://[^\s<>]+)>',
    caseSensitive: false,
  ).firstMatch(remaining);
  if (autolink != null && _isHttpUrl(autolink.group(1)!)) {
    return _InlineToken(
      kind: _InlineTokenKind.link,
      content: autolink.group(1)!,
      target: autolink.group(1)!,
      end: index + autolink.end,
    );
  }

  if (index > 0 && RegExp(r'[A-Za-z0-9_]').hasMatch(text[index - 1])) {
    return null;
  }
  final bareLink = RegExp(
    r'^https?://[^\s<>()]+',
    caseSensitive: false,
  ).firstMatch(remaining);
  if (bareLink == null) return null;
  var target = bareLink.group(0)!;
  while (target.isNotEmpty &&
      const [
        '.',
        ',',
        ';',
        ':',
        '!',
        '?',
        ']',
        '}',
      ].contains(target[target.length - 1])) {
    target = target.substring(0, target.length - 1);
  }
  if (!_isHttpUrl(target)) return null;
  return _InlineToken(
    kind: _InlineTokenKind.link,
    content: target,
    target: target,
    end: index + target.length,
  );
}

bool _isHttpUrl(String value) {
  final uri = Uri.tryParse(value);
  return uri != null &&
      (uri.scheme.toLowerCase() == 'http' ||
          uri.scheme.toLowerCase() == 'https') &&
      uri.host.isNotEmpty;
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
