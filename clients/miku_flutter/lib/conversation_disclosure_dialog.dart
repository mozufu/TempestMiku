part of 'conversation_screen.dart';

class _DisclosureConfirmDialog extends StatefulWidget {
  const _DisclosureConfirmDialog({
    required this.title,
    required this.summary,
    required this.details,
    required this.confirmLabel,
    required this.confirmKey,
    this.cancelLabel = '取消',
    this.destructive = false,
  });

  final String title;
  final String summary;
  final String details;
  final String confirmLabel;
  final Key confirmKey;
  final String cancelLabel;
  final bool destructive;

  @override
  State<_DisclosureConfirmDialog> createState() =>
      _DisclosureConfirmDialogState();
}

class _DisclosureConfirmDialogState extends State<_DisclosureConfirmDialog> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    return AlertDialog(
      title: Text(widget.title),
      content: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 420),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(widget.summary),
            InkWell(
              key: const Key('disclosure-confirm-toggle'),
              onTap: () => setState(() => _expanded = !_expanded),
              borderRadius: BorderRadius.circular(8),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 8),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      '了解更多',
                      style: TextStyle(
                        color: palette.miku,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    Icon(
                      _expanded
                          ? Icons.expand_less_rounded
                          : Icons.expand_more_rounded,
                      size: 18,
                      color: palette.miku,
                    ),
                  ],
                ),
              ),
            ),
            if (_expanded)
              Padding(
                padding: const EdgeInsets.only(bottom: 4),
                child: Text(
                  widget.details,
                  key: const Key('disclosure-confirm-details'),
                  style: Theme.of(
                    context,
                  ).textTheme.bodySmall?.copyWith(color: palette.muted),
                ),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: Text(widget.cancelLabel),
        ),
        FilledButton(
          key: widget.confirmKey,
          style:
              widget.destructive
                  ? FilledButton.styleFrom(
                    backgroundColor: Theme.of(context).colorScheme.error,
                    foregroundColor: Theme.of(context).colorScheme.onError,
                  )
                  : null,
          onPressed: () => Navigator.of(context).pop(true),
          child: Text(widget.confirmLabel),
        ),
      ],
    );
  }
}

Future<bool?> _showDisclosureConfirm(
  BuildContext context, {
  required String title,
  required String summary,
  required String details,
  required String confirmLabel,
  required Key confirmKey,
  String cancelLabel = '取消',
  bool destructive = false,
}) {
  return showDialog<bool>(
    context: context,
    barrierDismissible: false,
    builder:
        (context) => _DisclosureConfirmDialog(
          title: title,
          summary: summary,
          details: details,
          confirmLabel: confirmLabel,
          confirmKey: confirmKey,
          cancelLabel: cancelLabel,
          destructive: destructive,
        ),
  );
}
