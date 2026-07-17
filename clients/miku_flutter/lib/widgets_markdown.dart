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
