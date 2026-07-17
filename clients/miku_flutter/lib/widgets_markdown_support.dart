part of 'main.dart';

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
