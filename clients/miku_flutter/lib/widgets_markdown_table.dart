part of 'main.dart';

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
