part of 'main.dart';

class _MikuTopBar extends StatelessWidget {
  const _MikuTopBar({
    required this.tok,
    required this.copy,
    required this.status,
  });

  final _Tok tok;
  final _UiCopy copy;
  final String status;

  @override
  Widget build(BuildContext context) {
    final online =
        status == 'connected' || status == 'streaming' || status == 'complete';
    final statusColor =
        online
            ? tok.success
            : status == 'connecting'
            ? tok.cool
            : tok.warning;
    return Container(
      constraints: const BoxConstraints(minHeight: 52),
      padding: const EdgeInsets.fromLTRB(6, 4, 12, 4),
      decoration: BoxDecoration(
        color: tok.glass,
        border: Border(bottom: BorderSide(color: tok.glassBorder)),
      ),
      child: Row(
        children: [
          Builder(
            builder:
                (menuContext) => IconButton(
                  tooltip: copy.pick('Open menu', '開啟選單'),
                  onPressed: () => Scaffold.of(menuContext).openDrawer(),
                  icon: const Icon(Icons.menu_rounded),
                ),
          ),
          const SizedBox(width: 2),
          Expanded(
            child: Row(
              children: [
                Text(
                  'Miku',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 16.5,
                    fontWeight: FontWeight.w900,
                    letterSpacing: -0.35,
                  ),
                ),
                const SizedBox(width: 8),
                Container(
                  width: 7,
                  height: 7,
                  decoration: BoxDecoration(
                    color: statusColor,
                    shape: BoxShape.circle,
                  ),
                ),
                const SizedBox(width: 6),
                Flexible(
                  child: Text(
                    copy.statusLabel(status),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11.5,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _MikuComposer extends StatelessWidget {
  const _MikuComposer({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.controller,
    required this.sessionEnded,
    required this.isSending,
    required this.canSend,
    required this.sendError,
    required this.onChanged,
    required this.onSend,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final TextEditingController controller;
  final bool sessionEnded;
  final bool isSending;
  final bool canSend;
  final String sendError;
  final ValueChanged<String> onChanged;
  final VoidCallback onSend;

  @override
  Widget build(BuildContext context) {
    final canSubmit = canSend && !sessionEnded && !isSending;
    return LayoutBuilder(
      builder: (context, constraints) {
        return Container(
          padding: const EdgeInsets.fromLTRB(14, 8, 14, 12),
          decoration: BoxDecoration(
            color: tok.glass,
            border: Border(top: BorderSide(color: tok.glassBorder)),
          ),
          child: Center(
            child: SizedBox(
              width: math.min(constraints.maxWidth, 720),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (sendError.isNotEmpty) ...[
                    Semantics(
                      liveRegion: true,
                      label: sendError,
                      child: Container(
                        width: double.infinity,
                        margin: const EdgeInsets.only(bottom: 8),
                        padding: const EdgeInsets.symmetric(
                          horizontal: 12,
                          vertical: 9,
                        ),
                        decoration: BoxDecoration(
                          color: tok.danger.withValues(alpha: 0.1),
                          borderRadius: BorderRadius.circular(14),
                          border: Border.all(
                            color: tok.danger.withValues(alpha: 0.45),
                          ),
                        ),
                        child: Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Icon(
                              Icons.error_outline_rounded,
                              size: 19,
                              color: tok.danger,
                            ),
                            const SizedBox(width: 8),
                            Expanded(
                              child: Text(
                                sendError,
                                style: TextStyle(
                                  color: tok.text,
                                  fontSize: 12.5,
                                  height: 1.35,
                                  fontWeight: FontWeight.w600,
                                ),
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ],
                  Container(
                    decoration: BoxDecoration(
                      color: tok.raised,
                      border: Border.all(
                        color:
                            sendError.isEmpty
                                ? tok.border
                                : tok.danger.withValues(alpha: 0.7),
                      ),
                      borderRadius: BorderRadius.circular(20),
                      boxShadow: [
                        BoxShadow(
                          color: tok.glow,
                          blurRadius: 18,
                          offset: const Offset(0, 8),
                        ),
                      ],
                    ),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.end,
                      children: [
                        Expanded(
                          child: TextField(
                            controller: controller,
                            enabled: !sessionEnded,
                            readOnly: isSending,
                            style: TextStyle(
                              color: tok.text,
                              fontSize: 15,
                              height: 1.4,
                            ),
                            minLines: 1,
                            maxLines: 6,
                            keyboardType: TextInputType.multiline,
                            textInputAction: TextInputAction.send,
                            decoration: InputDecoration(
                              hintText:
                                  sessionEnded
                                      ? copy.sessionEndedHint
                                      : copy.messageHint,
                              filled: false,
                              border: InputBorder.none,
                              enabledBorder: InputBorder.none,
                              focusedBorder: InputBorder.none,
                              contentPadding: const EdgeInsets.fromLTRB(
                                16,
                                14,
                                8,
                                14,
                              ),
                            ),
                            onChanged: onChanged,
                            onSubmitted: (_) {
                              if (canSubmit) onSend();
                            },
                          ),
                        ),
                        Padding(
                          padding: const EdgeInsets.all(6),
                          child: Semantics(
                            button: true,
                            enabled: canSubmit,
                            label: copy.sendMessage,
                            child: Tooltip(
                              message:
                                  sessionEnded
                                      ? copy.sessionEnded
                                      : canSubmit
                                      ? copy.send
                                      : copy.typeMessage,
                              child: SizedBox.square(
                                dimension: 48,
                                child: IconButton.filled(
                                  onPressed: canSubmit ? onSend : null,
                                  style: IconButton.styleFrom(
                                    backgroundColor: accent,
                                    foregroundColor: _textOn(accent),
                                    disabledBackgroundColor: tok.border
                                        .withValues(alpha: 0.55),
                                    disabledForegroundColor: tok.muted,
                                    shape: RoundedRectangleBorder(
                                      borderRadius: BorderRadius.circular(16),
                                    ),
                                  ),
                                  icon:
                                      isSending
                                          ? SizedBox.square(
                                            dimension: 20,
                                            child: CircularProgressIndicator(
                                              strokeWidth: 2.2,
                                              color: _textOn(accent),
                                            ),
                                          )
                                          : const Icon(Icons.send, size: 21),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}
