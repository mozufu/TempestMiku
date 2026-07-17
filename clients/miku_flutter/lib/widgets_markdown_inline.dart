part of 'main.dart';

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
