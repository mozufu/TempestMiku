part of 'main.dart';

enum _ShareDestination { currentSession, newSession }

class _ShareImportDecision {
  const _ShareImportDecision({required this.text, required this.destination});

  final String text;
  final _ShareDestination destination;
}

class _ShareImportSheet extends StatefulWidget {
  const _ShareImportSheet({
    required this.content,
    required this.currentSessionAvailable,
    required this.tok,
    required this.copy,
  });

  final SharedContent content;
  final bool currentSessionAvailable;
  final _Tok tok;
  final _UiCopy copy;

  @override
  State<_ShareImportSheet> createState() => _ShareImportSheetState();
}

class _ShareImportSheetState extends State<_ShareImportSheet> {
  late final TextEditingController _controller;
  _ShareDestination? _destination;

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: widget.content.text);
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final copy = widget.copy;
    final isSelection = widget.content.source == SharedContentSource.selection;
    final keyboardInset = MediaQuery.viewInsetsOf(context).bottom;
    final canSend = _controller.text.trim().isNotEmpty && _destination != null;
    return AnimatedPadding(
      duration: const Duration(milliseconds: 180),
      padding: EdgeInsets.only(bottom: keyboardInset),
      child: SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(20, 12, 20, 20),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Center(
              child: Container(
                width: 40,
                height: 4,
                decoration: BoxDecoration(
                  color: tok.border,
                  borderRadius: BorderRadius.circular(99),
                ),
              ),
            ),
            const SizedBox(height: 18),
            Row(
              children: [
                Container(
                  width: 40,
                  height: 40,
                  decoration: BoxDecoration(
                    color: tok.accentSoft.withValues(alpha: 0.16),
                    borderRadius: BorderRadius.circular(12),
                  ),
                  child: Icon(
                    isSelection
                        ? Icons.text_fields_rounded
                        : Icons.share_outlined,
                    color: tok.accentSoft,
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        isSelection
                            ? copy.askMikuAboutThis
                            : copy.shareWithMiku,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 18,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                      const SizedBox(height: 3),
                      Text(
                        copy.shareReviewHelper,
                        style: TextStyle(color: tok.muted, fontSize: 12.5),
                      ),
                    ],
                  ),
                ),
              ],
            ),
            if (widget.content.truncated) ...[
              const SizedBox(height: 14),
              Container(
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: tok.warning.withValues(alpha: 0.12),
                  border: Border.all(
                    color: tok.warning.withValues(alpha: 0.35),
                  ),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Row(
                  children: [
                    Icon(
                      Icons.warning_amber_rounded,
                      color: tok.warning,
                      size: 19,
                    ),
                    const SizedBox(width: 9),
                    Expanded(
                      child: Text(
                        copy.shareTruncated,
                        style: TextStyle(color: tok.text, fontSize: 12.5),
                      ),
                    ),
                  ],
                ),
              ),
            ],
            const SizedBox(height: 16),
            TextField(
              key: const ValueKey('shareImportEditor'),
              controller: _controller,
              minLines: 4,
              maxLines: 8,
              maxLength: maxSharedTextLength,
              style: TextStyle(color: tok.text, fontSize: 14),
              decoration: InputDecoration(
                labelText: isSelection ? copy.selectedText : copy.sharedContent,
                helperText:
                    isSelection
                        ? copy.selectedFromAndroid
                        : copy.sharedFromAndroid,
                filled: true,
                fillColor: tok.raised,
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(14),
                  borderSide: BorderSide(color: tok.border),
                ),
                enabledBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(14),
                  borderSide: BorderSide(color: tok.border),
                ),
              ),
              onChanged: (_) => setState(() {}),
            ),
            const SizedBox(height: 12),
            Text(
              copy.sendTo,
              style: TextStyle(
                color: tok.text,
                fontSize: 12.5,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 8),
            SegmentedButton<_ShareDestination>(
              key: const ValueKey('shareImportDestination'),
              segments: [
                ButtonSegment(
                  value: _ShareDestination.currentSession,
                  enabled: widget.currentSessionAvailable,
                  icon: const Icon(Icons.chat_bubble_outline, size: 17),
                  label: Text(copy.currentChat),
                ),
                ButtonSegment(
                  value: _ShareDestination.newSession,
                  icon: const Icon(Icons.add_comment_outlined, size: 17),
                  label: Text(copy.newChat),
                ),
              ],
              selected: {if (_destination != null) _destination!},
              emptySelectionAllowed: true,
              showSelectedIcon: false,
              onSelectionChanged: (selection) {
                setState(() => _destination = selection.first);
              },
            ),
            const SizedBox(height: 18),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                TextButton(
                  onPressed: () => Navigator.pop(context),
                  child: Text(copy.cancel),
                ),
                const SizedBox(width: 8),
                FilledButton.icon(
                  key: const ValueKey('shareImportSend'),
                  onPressed:
                      canSend
                          ? () => Navigator.pop(
                            context,
                            _ShareImportDecision(
                              text: _controller.text.trim(),
                              destination: _destination!,
                            ),
                          )
                          : null,
                  icon: const Icon(Icons.send, size: 17),
                  label: Text(copy.sendToMiku),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
