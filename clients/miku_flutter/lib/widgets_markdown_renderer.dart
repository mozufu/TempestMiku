part of 'main.dart';

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
