part of 'main.dart';

// ─── Small reusable widgets ────────────────────────────────────────────────────

bool _reducedMotion(BuildContext context) {
  final media = MediaQuery.maybeOf(context);
  return media?.disableAnimations == true ||
      media?.accessibleNavigation == true;
}

class _TokIconBtn extends StatelessWidget {
  const _TokIconBtn({
    required this.tok,
    required this.icon,
    required this.tooltip,
    required this.semanticLabel,
    required this.onTap,
  });

  final _Tok tok;
  final IconData icon;
  final String tooltip;
  final String semanticLabel;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: Semantics(
        button: true,
        enabled: onTap != null,
        label: semanticLabel,
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(10),
            focusColor: tok.focus.withOpacity(0.2),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(10),
              ),
              child: Icon(icon, color: tok.muted, size: 17),
            ),
          ),
        ),
      ),
    );
  }
}

class _ModeDropMenuButton extends StatelessWidget {
  const _ModeDropMenuButton({
    required this.tok,
    required this.copy,
    required this.mode,
    required this.accent,
    required this.locked,
    required this.onTap,
    this.compact = false,
  });

  final _Tok tok;
  final _UiCopy copy;
  final _Mode mode;
  final Color accent;
  final bool locked;
  final VoidCallback onTap;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final borderColor = locked ? accent.withOpacity(0.58) : tok.border;
    final bg = locked ? accent.withOpacity(0.12) : tok.surface.withOpacity(0.6);
    final label = copy.modeChipLabel(mode, locked);
    return Tooltip(
      message: locked ? copy.modeLocked : copy.switchMode,
      child: Semantics(
        button: true,
        label: locked ? copy.currentModeLocked(mode) : copy.currentMode(mode),
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(999),
            focusColor: tok.focus.withOpacity(0.18),
            child: Container(
              height: 40,
              constraints: BoxConstraints(
                minWidth: compact ? 62 : 72,
                maxWidth: compact ? 90 : 128,
              ),
              padding: const EdgeInsets.symmetric(horizontal: 10),
              decoration: BoxDecoration(
                color: bg,
                border: Border.all(color: borderColor),
                borderRadius: BorderRadius.circular(999),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    locked ? Icons.lock : mode.icon,
                    color: locked ? accent : tok.muted,
                    size: 14,
                  ),
                  const SizedBox(width: 5),
                  Flexible(
                    child: Text(
                      label,
                      overflow: TextOverflow.ellipsis,
                      maxLines: 1,
                      style: TextStyle(
                        color: locked ? accent : tok.muted,
                        fontSize: 11,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                  ),
                  const SizedBox(width: 2),
                  Icon(Icons.arrow_drop_down, color: tok.muted, size: 16),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _ConnectionBadge extends StatefulWidget {
  const _ConnectionBadge({
    required this.status,
    required this.tok,
    required this.copy,
    this.compact = false,
  });

  final String status;
  final _Tok tok;
  final _UiCopy copy;
  final bool compact;

  @override
  State<_ConnectionBadge> createState() => _ConnectionBadgeState();
}

class _ConnectionBadgeState extends State<_ConnectionBadge>
    with SingleTickerProviderStateMixin {
  late final AnimationController _pulse;

  @override
  void initState() {
    super.initState();
    _pulse = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2200),
    )..repeat();
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final isLive = widget.status == 'connected' ||
        widget.status == 'streaming' ||
        widget.status == 'complete';
    final dotColor = isLive ? tok.success : tok.warning;
    final label = widget.copy.statusLabel(widget.status);
    final reduceMotion = _reducedMotion(context);

    Widget dot() {
      return AnimatedBuilder(
        animation: _pulse,
        builder: (_, __) {
          final t = _pulse.value * math.pi * 2;
          final opacity =
              reduceMotion ? 1.0 : (math.sin(t) * 0.34 + 0.66).clamp(0.32, 1.0);
          return Opacity(
            opacity: isLive ? opacity : 0.55,
            child: Container(
              width: 7,
              height: 7,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: dotColor,
              ),
            ),
          );
        },
      );
    }

    return Tooltip(
      message: label,
      child: Semantics(
        label: widget.copy.connectionStatus(label),
        child: Container(
          width: widget.compact ? 40 : null,
          height: widget.compact ? 40 : null,
          padding: widget.compact
              ? EdgeInsets.zero
              : const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          decoration: BoxDecoration(
            border: Border.all(color: tok.border),
            borderRadius: BorderRadius.circular(999),
          ),
          child: widget.compact
              ? Center(child: dot())
              : Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    dot(),
                    const SizedBox(width: 6),
                    Text(
                      label,
                      style: TextStyle(
                        color: tok.muted,
                        fontSize: 11,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                  ],
                ),
        ),
      ),
    );
  }
}

class _LanguageToggle extends StatelessWidget {
  const _LanguageToggle({
    required this.tok,
    required this.copy,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: copy.languageTooltip,
      child: Semantics(
        button: true,
        label: copy.languageSemantic,
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(10),
            focusColor: tok.focus.withOpacity(0.2),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(10),
              ),
              child: Center(
                child: Text(
                  copy.code,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11,
                    fontWeight: FontWeight.w900,
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState({
    required this.tok,
    required this.accent,
    required this.status,
    required this.copy,
  });

  final _Tok tok;
  final Color accent;
  final String status;
  final _UiCopy copy;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 92, 4, 28),
      child: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 360),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 44,
                height: 44,
                decoration: BoxDecoration(
                  color: accent,
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Icon(Icons.smart_toy, color: _textOn(accent), size: 22),
              ),
              const SizedBox(height: 14),
              Text(
                copy.emptyTitle,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 16,
                  fontWeight: FontWeight.w800,
                ),
              ),
              const SizedBox(height: 6),
              Text(
                copy.statusLabel(status),
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _RoundLabel extends StatelessWidget {
  const _RoundLabel({
    required this.tok,
    required this.copy,
    required this.index,
  });

  final _Tok tok;
  final _UiCopy copy;
  final int index;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Text(
          copy.round(index),
          style: TextStyle(
            color: tok.muted,
            fontSize: 11,
            fontWeight: FontWeight.w800,
          ),
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Container(
            height: 0.5,
            color: tok.border.withOpacity(0.7),
          ),
        ),
      ],
    );
  }
}

class _UserBubble extends StatelessWidget {
  const _UserBubble({
    required this.tok,
    required this.text,
    required this.accent,
  });

  final _Tok tok;
  final String text;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.centerRight,
      child: Container(
        constraints: const BoxConstraints(maxWidth: 280),
        padding: const EdgeInsets.fromLTRB(13, 10, 13, 10),
        decoration: BoxDecoration(
          color: accent,
          borderRadius: const BorderRadius.only(
            topLeft: Radius.circular(15),
            topRight: Radius.circular(15),
            bottomLeft: Radius.circular(15),
            bottomRight: Radius.circular(5),
          ),
        ),
        child: Text(
          text,
          style: TextStyle(
            color: _textOn(accent),
            fontSize: 14,
            fontWeight: FontWeight.w500,
            height: 1.5,
          ),
        ),
      ),
    );
  }
}

class _MikuBubble extends StatelessWidget {
  const _MikuBubble({
    required this.tok,
    required this.copy,
    required this.text,
    required this.accent,
    required this.resources,
    required this.onOpenResource,
    this.isStreaming = false,
  });

  final _Tok tok;
  final _UiCopy copy;
  final String text;
  final Color accent;
  final List<String> resources;
  final void Function(String) onOpenResource;
  final bool isStreaming;

  @override
  Widget build(BuildContext context) {
    final iconColor = _textOn(accent);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 30,
          height: 30,
          decoration: BoxDecoration(
            color: accent,
            borderRadius: BorderRadius.circular(9),
          ),
          child: Icon(Icons.smart_toy, color: iconColor, size: 17),
        ),
        const SizedBox(width: 9),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Text(
                    'Miku',
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 14,
                      fontWeight: FontWeight.w800,
                      letterSpacing: -0.3,
                    ),
                  ),
                  if (isStreaming) ...[
                    const SizedBox(width: 6),
                    _PulsingDot(color: accent),
                  ],
                ],
              ),
              const SizedBox(height: 5),
              _MarkdownMessage(
                tok: tok,
                accent: accent,
                text: text,
              ),
              if (resources.isNotEmpty) ...[
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: resources
                      .map(
                        (uri) => Semantics(
                          button: true,
                          label: copy.openResource(uri),
                          child: Material(
                            color: Colors.transparent,
                            child: InkWell(
                              key: ValueKey('resource:$uri'),
                              onTap: () => onOpenResource(uri),
                              borderRadius: BorderRadius.circular(9),
                              focusColor: tok.focus.withOpacity(0.18),
                              child: Container(
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 10,
                                  vertical: 7,
                                ),
                                decoration: BoxDecoration(
                                  color: tok.surface,
                                  border: Border.all(color: tok.border),
                                  borderRadius: BorderRadius.circular(9),
                                ),
                                child: Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    Icon(
                                      Icons.insert_drive_file_outlined,
                                      size: 13,
                                      color: accent,
                                    ),
                                    const SizedBox(width: 6),
                                    Text(
                                      uri,
                                      style: TextStyle(
                                        color: accent,
                                        fontSize: 11.5,
                                        fontWeight: FontWeight.w700,
                                        fontFamily: 'monospace',
                                      ),
                                    ),
                                    const SizedBox(width: 4),
                                    Icon(
                                      Icons.open_in_new,
                                      size: 11,
                                      color: tok.muted,
                                    ),
                                  ],
                                ),
                              ),
                            ),
                          ),
                        ),
                      )
                      .toList(),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

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
  var inCode = false;

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

  for (final raw in lines) {
    final line = raw.trimRight();
    final trimmed = line.trim();
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
  final pattern = RegExp(r'(\*\*[^*]+\*\*|`[^`]+`)');
  var index = 0;
  for (final match in pattern.allMatches(text)) {
    if (match.start > index) {
      spans.add(TextSpan(
        text: text.substring(index, match.start),
        style: baseStyle,
      ));
    }
    final token = match.group(0)!;
    if (token.startsWith('**')) {
      spans.add(TextSpan(
        text: token.substring(2, token.length - 2),
        style: baseStyle.copyWith(fontWeight: FontWeight.w900),
      ));
    } else {
      spans.add(TextSpan(
        text: token.substring(1, token.length - 1),
        style: baseStyle.copyWith(
          color: accent,
          fontFamily: 'monospace',
          fontWeight: FontWeight.w700,
          backgroundColor: tok.surface.withOpacity(0.85),
        ),
      ));
    }
    index = match.end;
  }
  if (index < text.length) {
    spans.add(TextSpan(text: text.substring(index), style: baseStyle));
  }
  return spans;
}

class _PulsingDot extends StatefulWidget {
  const _PulsingDot({required this.color});

  final Color color;

  @override
  State<_PulsingDot> createState() => _PulsingDotState();
}

class _PulsingDotState extends State<_PulsingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _c;

  @override
  void initState() {
    super.initState();
    _c = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1800),
    )..repeat();
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_reducedMotion(context)) {
      return Container(
        width: 6,
        height: 6,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: widget.color,
        ),
      );
    }
    return AnimatedBuilder(
      animation: _c,
      builder: (_, __) {
        final opacity =
            (math.sin(_c.value * math.pi * 2) * 0.34 + 0.66).clamp(0.32, 1.0);
        return Opacity(
          opacity: opacity,
          child: Container(
            width: 6,
            height: 6,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: widget.color,
            ),
          ),
        );
      },
    );
  }
}

class _TypingIndicator extends StatelessWidget {
  const _TypingIndicator({
    required this.tok,
    required this.accent,
    required this.anim,
  });

  final _Tok tok;
  final Color accent;
  final AnimationController anim;

  @override
  Widget build(BuildContext context) {
    final reduceMotion = _reducedMotion(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 30,
          height: 30,
          decoration: BoxDecoration(
            color: accent,
            borderRadius: BorderRadius.circular(9),
          ),
          child: Icon(Icons.smart_toy, color: _textOn(accent), size: 17),
        ),
        const SizedBox(width: 9),
        Padding(
          padding: const EdgeInsets.only(top: 7),
          child: AnimatedBuilder(
            animation: anim,
            builder: (_, __) => Row(
              mainAxisSize: MainAxisSize.min,
              children: List.generate(3, (i) {
                final phase =
                    reduceMotion ? 0.0 : (anim.value - i * 0.18) % 1.0;
                final opacity = reduceMotion
                    ? 0.85
                    : (math.sin(phase * math.pi * 2) * 0.4 + 0.6)
                        .clamp(0.25, 1.0);
                final dy = reduceMotion
                    ? 0.0
                    : (math.sin(phase * math.pi * 2) * -2.0).clamp(-2.0, 0.0);
                return Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 2),
                  child: Transform.translate(
                    offset: Offset(0, dy),
                    child: Opacity(
                      opacity: opacity,
                      child: Container(
                        width: 6,
                        height: 6,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: tok.muted,
                        ),
                      ),
                    ),
                  ),
                );
              }),
            ),
          ),
        ),
      ],
    );
  }
}

class _AgentStatusBar extends StatelessWidget {
  const _AgentStatusBar({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.anim,
    required this.roundIndex,
    required this.agents,
    required this.activities,
    required this.expanded,
    required this.onTap,
    required this.onOpenResource,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final AnimationController anim;
  final int roundIndex;
  final List<_AgentStatus> agents;
  final List<_ActivityItem> activities;
  final bool expanded;
  final VoidCallback onTap;
  final void Function(String) onOpenResource;

  @override
  Widget build(BuildContext context) {
    final running = agents.where((agent) => agent.isRunning).length;
    final stopped = agents.length - running;
    final visibleAgents =
        agents.length > 4 ? agents.sublist(agents.length - 4) : agents;
    final fallback = _runtimeFallback();
    return Semantics(
      button: true,
      label: copy.openAgentActivity,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('agent-activity:$roundIndex'),
          onTap: onTap,
          borderRadius: BorderRadius.circular(10),
          focusColor: tok.focus.withOpacity(0.18),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  color: tok.surface,
                  border: Border.all(color: tok.border),
                  borderRadius: BorderRadius.circular(9),
                ),
                child:
                    Icon(Icons.account_tree_outlined, color: accent, size: 16),
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Container(
                  padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
                  decoration: BoxDecoration(
                    color: tok.surface.withOpacity(0.78),
                    border: Border.all(color: tok.border.withOpacity(0.82)),
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Icon(Icons.route_outlined, color: accent, size: 14),
                          const SizedBox(width: 6),
                          Expanded(
                            child: Text(
                              agents.isEmpty
                                  ? copy.runtimeStatus
                                  : copy.agentsSummary(running, stopped),
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                color: tok.text,
                                fontSize: 12,
                                fontWeight: FontWeight.w800,
                              ),
                            ),
                          ),
                          const SizedBox(width: 8),
                          Text(
                            copy.events(activities.length),
                            style: TextStyle(
                              color: tok.muted,
                              fontSize: 10.5,
                              fontWeight: FontWeight.w800,
                            ),
                          ),
                          const SizedBox(width: 4),
                          Icon(Icons.open_in_full, color: tok.muted, size: 12),
                        ],
                      ),
                      const SizedBox(height: 7),
                      if (visibleAgents.isNotEmpty)
                        for (final agent in visibleAgents) ...[
                          _AgentStatusLine(
                            tok: tok,
                            copy: copy,
                            accent: accent,
                            anim: anim,
                            agent: agent,
                          ),
                          if (agent != visibleAgents.last)
                            const SizedBox(height: 5),
                        ]
                      else
                        _RuntimeStatusLine(
                          tok: tok,
                          copy: copy,
                          accent: accent,
                          anim: anim,
                          item: fallback,
                        ),
                      if (expanded) ...[
                        const SizedBox(height: 9),
                        Container(
                          height: 0.5,
                          color: tok.border.withOpacity(0.72),
                        ),
                        const SizedBox(height: 9),
                        for (var i = 0; i < activities.length; i++) ...[
                          _ActivityRow(
                            tok: tok,
                            copy: copy,
                            accent: accent,
                            item: activities[i],
                            onOpenResource: onOpenResource,
                          ),
                          if (i != activities.length - 1) ...[
                            const SizedBox(height: 8),
                            Container(
                              height: 0.5,
                              color: tok.border.withOpacity(0.55),
                            ),
                            const SizedBox(height: 8),
                          ],
                        ],
                      ],
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  _ActivityItem _runtimeFallback() {
    for (final item in activities.reversed) {
      if (item.kind == 'cell' || item.kind == 'tool') return item;
    }
    return activities.last;
  }
}

// ─── Reasoning / chain-of-thought trace ───────────────────────────────────────

/// A collapsible rendering of the private chain-of-thought a provider returned
/// alongside the answer. The parent round controls whether it stays expanded.
class _ThinkingTrace extends StatefulWidget {
  const _ThinkingTrace({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.text,
    required this.expanded,
    required this.isStreaming,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final String text;
  final bool expanded;
  final bool isStreaming;

  @override
  State<_ThinkingTrace> createState() => _ThinkingTraceState();
}

class _ThinkingTraceState extends State<_ThinkingTrace> {
  late bool _expanded = widget.expanded;
  bool _userToggled = false;

  @override
  void didUpdateWidget(covariant _ThinkingTrace oldWidget) {
    super.didUpdateWidget(oldWidget);
    // Follow the parent's expanded state for live open/close, unless the user has
    // manually toggled this trace.
    if (!_userToggled && oldWidget.expanded != widget.expanded) {
      _expanded = widget.expanded;
    }
  }

  void _toggle() {
    setState(() {
      _expanded = !_expanded;
      _userToggled = true;
    });
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final accent = widget.accent;
    return Semantics(
      button: true,
      label: widget.copy.thinkingTrace,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: _toggle,
          borderRadius: BorderRadius.circular(10),
          focusColor: tok.focus.withOpacity(0.18),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  color: tok.surface,
                  border: Border.all(color: tok.border),
                  borderRadius: BorderRadius.circular(9),
                ),
                child: Icon(
                  Icons.psychology_outlined,
                  color: accent,
                  size: 16,
                ),
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Container(
                  padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
                  decoration: BoxDecoration(
                    color: tok.surface.withOpacity(0.78),
                    border: Border.all(color: tok.border.withOpacity(0.82)),
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Icon(Icons.psychology_outlined,
                              color: accent, size: 14),
                          const SizedBox(width: 6),
                          Expanded(
                            child: Text(
                              widget.copy.thinking,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                color: tok.muted,
                                fontSize: 12,
                                fontWeight: FontWeight.w800,
                                letterSpacing: 0.2,
                              ),
                            ),
                          ),
                          if (widget.isStreaming) ...[
                            _PulsingDot(color: accent),
                            const SizedBox(width: 6),
                          ],
                          Icon(
                            _expanded ? Icons.expand_less : Icons.expand_more,
                            color: tok.muted,
                            size: 16,
                          ),
                        ],
                      ),
                      if (_expanded) ...[
                        const SizedBox(height: 8),
                        Container(
                          height: 0.5,
                          color: tok.border.withOpacity(0.72),
                        ),
                        const SizedBox(height: 8),
                        Container(
                          constraints: const BoxConstraints(maxHeight: 240),
                          child: SingleChildScrollView(
                            child: SelectableText(
                              widget.text,
                              style: TextStyle(
                                color: tok.muted,
                                fontSize: 12.5,
                                height: 1.55,
                                fontFamily: 'monospace',
                                fontStyle: FontStyle.italic,
                              ),
                            ),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _AgentStatusLine extends StatelessWidget {
  const _AgentStatusLine({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.anim,
    required this.agent,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final AnimationController anim;
  final _AgentStatus agent;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        _StatusGlyph(tok: tok, accent: accent, anim: anim, state: agent.state),
        const SizedBox(width: 7),
        Expanded(
          child: Text(
            '${agent.role} agent · ${agent.id}',
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: tok.text,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
        ),
        Text(
          copy.stateLabel(agent.state),
          style: TextStyle(
            color: agent.isRunning ? accent : tok.muted,
            fontSize: 10.5,
            fontWeight: FontWeight.w800,
          ),
        ),
      ],
    );
  }
}

class _RuntimeStatusLine extends StatelessWidget {
  const _RuntimeStatusLine({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.anim,
    required this.item,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final AnimationController anim;
  final _ActivityItem item;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        _StatusGlyph(tok: tok, accent: accent, anim: anim, state: item.state),
        const SizedBox(width: 7),
        Expanded(
          child: Text(
            item.title,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: tok.text,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
        ),
        Text(
          copy.stateLabel(item.state),
          style: TextStyle(
            color: item.state == _ActivityState.running ? accent : tok.muted,
            fontSize: 10.5,
            fontWeight: FontWeight.w800,
          ),
        ),
      ],
    );
  }
}

class _StatusGlyph extends StatelessWidget {
  const _StatusGlyph({
    required this.tok,
    required this.accent,
    required this.anim,
    required this.state,
  });

  final _Tok tok;
  final Color accent;
  final AnimationController anim;
  final _ActivityState state;

  @override
  Widget build(BuildContext context) {
    if (state == _ActivityState.running) {
      if (_reducedMotion(context)) {
        return Container(
          width: 14,
          height: 14,
          decoration: BoxDecoration(
            color: accent,
            shape: BoxShape.circle,
          ),
        );
      }
      return AnimatedBuilder(
        animation: anim,
        builder: (_, __) {
          final opacity = (math.sin(anim.value * math.pi * 2) * 0.34 + 0.66)
              .clamp(0.34, 1.0);
          return Opacity(
            opacity: opacity,
            child: Container(
              width: 14,
              height: 14,
              decoration: BoxDecoration(
                color: accent,
                shape: BoxShape.circle,
              ),
            ),
          );
        },
      );
    }
    final icon = switch (state) {
      _ActivityState.failed => Icons.error_outline,
      _ActivityState.done => Icons.stop_circle_outlined,
      _ActivityState.info => Icons.info_outline,
      _ActivityState.running => Icons.circle,
    };
    final color = switch (state) {
      _ActivityState.failed => tok.danger,
      _ActivityState.done => tok.success,
      _ActivityState.info => tok.muted,
      _ActivityState.running => accent,
    };
    return Icon(icon, color: color, size: 15);
  }
}

class _ActivityRow extends StatelessWidget {
  const _ActivityRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.item,
    required this.onOpenResource,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final _ActivityItem item;
  final void Function(String) onOpenResource;

  @override
  Widget build(BuildContext context) {
    final stateColor = _stateColor(item.state);
    final detail = _trimActivityDetail(item.detail);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 22,
          height: 22,
          decoration: BoxDecoration(
            color: stateColor.withOpacity(0.12),
            border: Border.all(color: stateColor.withOpacity(0.38)),
            borderRadius: BorderRadius.circular(7),
          ),
          child: Icon(item.icon, size: 13, color: stateColor),
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                item.title,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 12.2,
                  fontWeight: FontWeight.w800,
                ),
              ),
              if (detail.isNotEmpty) ...[
                const SizedBox(height: 3),
                Text(
                  detail,
                  maxLines: item.monospace ? 5 : 3,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: item.monospace ? 11 : 11.5,
                    fontWeight: FontWeight.w600,
                    height: 1.35,
                    fontFamily: item.monospace ? 'monospace' : null,
                  ),
                ),
              ],
              if (item.resourceUris.isNotEmpty) ...[
                const SizedBox(height: 6),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: item.resourceUris
                      .map(
                        (uri) => Semantics(
                          button: true,
                          label: copy.openActivityResource(uri),
                          child: Material(
                            color: Colors.transparent,
                            child: InkWell(
                              key: ValueKey('activity-resource:$uri'),
                              onTap: () => onOpenResource(uri),
                              borderRadius: BorderRadius.circular(8),
                              focusColor: tok.focus.withOpacity(0.18),
                              child: Container(
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 8,
                                  vertical: 6,
                                ),
                                decoration: BoxDecoration(
                                  color: tok.surface,
                                  border: Border.all(color: tok.border),
                                  borderRadius: BorderRadius.circular(8),
                                ),
                                child: Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    Icon(
                                      Icons.insert_drive_file_outlined,
                                      size: 12,
                                      color: accent,
                                    ),
                                    const SizedBox(width: 5),
                                    Text(
                                      uri,
                                      style: TextStyle(
                                        color: accent,
                                        fontSize: 10.8,
                                        fontWeight: FontWeight.w800,
                                        fontFamily: 'monospace',
                                      ),
                                    ),
                                    const SizedBox(width: 4),
                                    Icon(
                                      Icons.open_in_new,
                                      size: 10,
                                      color: tok.muted,
                                    ),
                                  ],
                                ),
                              ),
                            ),
                          ),
                        ),
                      )
                      .toList(),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Color _stateColor(_ActivityState state) => switch (state) {
        _ActivityState.running => accent,
        _ActivityState.done => tok.success,
        _ActivityState.failed => tok.danger,
        _ActivityState.info => tok.muted,
      };
}

String _trimActivityDetail(String detail) {
  final cleaned = detail
      .trim()
      .split('\n')
      .map((line) => line.trimRight())
      .where((line) => line.trim().isNotEmpty)
      .join('\n');
  if (cleaned.length <= 520) return cleaned;
  return '${cleaned.substring(0, 517)}...';
}

class _ApprovalCard extends StatelessWidget {
  const _ApprovalCard({
    required this.tok,
    required this.copy,
    required this.approval,
    required this.accent,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final ApprovalPrompt approval;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: copy.pendingApprovalSemantics(approval.action),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('approval:${approval.action}'),
          onTap: onTap,
          borderRadius: BorderRadius.circular(13),
          focusColor: tok.focus.withOpacity(0.18),
          child: Container(
            padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
            decoration: BoxDecoration(
              color: tok.warning.withOpacity(0.11),
              border: Border.all(color: tok.warning.withOpacity(0.52)),
              borderRadius: BorderRadius.circular(13),
            ),
            child: Row(
              children: [
                Container(
                  width: 34,
                  height: 34,
                  decoration: BoxDecoration(
                    color: tok.warning,
                    borderRadius: BorderRadius.circular(9),
                  ),
                  child: Icon(
                    Icons.warning_amber_rounded,
                    color: _textOn(tok.warning),
                    size: 18,
                  ),
                ),
                const SizedBox(width: 11),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        copy.pendingApproval(approval.action),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 13,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        copy.tapForDetails,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 11.5,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ],
                  ),
                ),
                Icon(Icons.chevron_right, color: tok.muted, size: 18),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _MemoryProposalCard extends StatelessWidget {
  const _MemoryProposalCard({
    required this.tok,
    required this.copy,
    required this.proposal,
    required this.approval,
    required this.accent,
    required this.onApprove,
    required this.onDeny,
  });

  final _Tok tok;
  final _UiCopy copy;
  final MemoryWriteProposal proposal;
  final ApprovalPrompt? approval;
  final Color accent;
  final VoidCallback? onApprove;
  final VoidCallback? onDeny;

  @override
  Widget build(BuildContext context) {
    final hasApproval = approval != null;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
      decoration: BoxDecoration(
        color: tok.surface,
        border: Border.all(color: accent.withOpacity(0.46)),
        borderRadius: BorderRadius.circular(13),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              color: accent,
              borderRadius: BorderRadius.circular(9),
            ),
            child: Icon(Icons.memory, color: _textOn(accent), size: 18),
          ),
          const SizedBox(width: 11),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        copy.memoryProposal,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 12.8,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    Container(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 7,
                        vertical: 2,
                      ),
                      decoration: BoxDecoration(
                        color: accent.withOpacity(0.1),
                        borderRadius: BorderRadius.circular(999),
                      ),
                      child: Text(
                        hasApproval ? copy.pending : copy.syncing,
                        style: TextStyle(
                          color: accent,
                          fontSize: 10.5,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 5),
                Text(
                  proposal.displayText,
                  maxLines: 3,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    height: 1.42,
                  ),
                ),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.label_outline,
                      text: proposal.kindLabel,
                    ),
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.folder_outlined,
                      text: copy.scopeChip(proposal.scopeLabel),
                    ),
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.history,
                      text: copy.provenanceChip(proposal.provenanceText),
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                Row(
                  children: [
                    _ProposalActionButton(
                      tok: tok,
                      icon: Icons.close,
                      label: copy.deny,
                      disabledLabel: copy.waitingForApproval,
                      waitingLabel: copy.waiting,
                      onTap: onDeny,
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: _ProposalActionButton(
                        tok: tok,
                        icon: Icons.check,
                        label: copy.saveMemory,
                        disabledLabel: copy.waitingForApproval,
                        waitingLabel: copy.waiting,
                        accent: accent,
                        onTap: onApprove,
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _ProposalChip extends StatelessWidget {
  const _ProposalChip({
    required this.tok,
    required this.icon,
    required this.text,
  });

  final _Tok tok;
  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 12, color: tok.muted),
          const SizedBox(width: 5),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 210),
            child: Text(
              text,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: tok.muted,
                fontSize: 10.8,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ProposalActionButton extends StatelessWidget {
  const _ProposalActionButton({
    required this.tok,
    required this.icon,
    required this.label,
    required this.disabledLabel,
    required this.waitingLabel,
    required this.onTap,
    this.accent,
  });

  final _Tok tok;
  final IconData icon;
  final String label;
  final String disabledLabel;
  final String waitingLabel;
  final VoidCallback? onTap;
  final Color? accent;

  @override
  Widget build(BuildContext context) {
    final active = onTap != null;
    final bg = accent ?? tok.bg;
    final fg = accent == null ? tok.text : _textOn(accent!);
    final border = accent ?? tok.border;
    return Opacity(
      opacity: active ? 1 : 0.54,
      child: Semantics(
        button: true,
        enabled: active,
        label: active ? label : disabledLabel,
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(10),
            focusColor: tok.focus.withOpacity(0.18),
            child: Container(
              height: 38,
              padding: const EdgeInsets.symmetric(horizontal: 10),
              decoration: BoxDecoration(
                color: bg,
                border: Border.all(color: border),
                borderRadius: BorderRadius.circular(10),
              ),
              child: Center(
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(icon, color: fg, size: 15),
                    const SizedBox(width: 6),
                    Flexible(
                      child: Text(
                        active ? label : waitingLabel,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: fg,
                          fontSize: 12.3,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
