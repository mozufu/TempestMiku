part of 'conversation_screen.dart';

class _Composer extends StatelessWidget {
  const _Composer({
    required this.controller,
    required this.focusNode,
    required this.enabled,
    required this.disabledHint,
    required this.sending,
    required this.onSend,
    required this.voiceVisible,
    required this.voiceReady,
    required this.voiceRecording,
    required this.voiceProcessing,
    required this.voiceSummary,
    required this.voiceError,
    required this.onVoiceAction,
    required this.onVoiceCancel,
  });

  final TextEditingController controller;
  final FocusNode focusNode;
  final bool enabled;
  final String disabledHint;
  final bool sending;
  final VoidCallback onSend;
  final bool voiceVisible;
  final bool voiceReady;
  final bool voiceRecording;
  final bool voiceProcessing;
  final String voiceSummary;
  final String? voiceError;
  final VoidCallback onVoiceAction;
  final VoidCallback onVoiceCancel;

  @override
  Widget build(BuildContext context) {
    final voiceBusy = voiceRecording || voiceProcessing;
    final sendReady = enabled && !sending && !voiceBusy;
    final canStartVoice = enabled && !sending && voiceReady && !voiceProcessing;
    final colors = Theme.of(context).colorScheme;
    KeyEventResult handleComposerKey(FocusNode node, KeyEvent event) {
      if (!kIsWeb || event is! KeyDownEvent) return KeyEventResult.ignored;
      final isEnter =
          event.logicalKey == LogicalKeyboardKey.enter ||
          event.logicalKey == LogicalKeyboardKey.numpadEnter;
      if (!isEnter ||
          HardwareKeyboard.instance.isShiftPressed ||
          !sendReady ||
          controller.text.trim().isEmpty) {
        return KeyEventResult.ignored;
      }
      onSend();
      return KeyEventResult.handled;
    }

    return Padding(
      padding: const EdgeInsets.fromLTRB(0, 10, 0, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (voiceVisible && (voiceBusy || voiceError != null)) ...[
            Semantics(
              liveRegion: true,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(14, 0, 12, 7),
                child: Row(
                  children: [
                    Icon(
                      voiceRecording
                          ? Icons.fiber_manual_record_rounded
                          : voiceError != null
                          ? Icons.error_outline_rounded
                          : Icons.graphic_eq_rounded,
                      size: 16,
                      color:
                          voiceError != null
                              ? colors.error
                              : voiceRecording
                              ? colors.error
                              : colors.primary,
                    ),
                    const SizedBox(width: 7),
                    Expanded(
                      child: Text(
                        voiceError ??
                            (voiceRecording
                                ? '錄音中 · 點停止後才會開始轉寫'
                                : '正在轉寫 · 完成後會先開啟可編輯草稿'),
                        key: const Key('voice-composer-status'),
                        style: Theme.of(context).textTheme.bodySmall?.copyWith(
                          color:
                              voiceError != null
                                  ? colors.error
                                  : TmTokens.of(context).muted,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
          Semantics(
            textField: true,
            label: '告訴 Miku',
            child: Focus(
              onKeyEvent: handleComposerKey,
              child: TextField(
                key: const Key('conversation-composer'),
                controller: controller,
                focusNode: focusNode,
                enabled: enabled,
                minLines: 1,
                maxLines: 6,
                textCapitalization: TextCapitalization.sentences,
                keyboardType: TextInputType.multiline,
                textInputAction: TextInputAction.newline,
                decoration: InputDecoration(
                  hintText: enabled ? '告訴 Miku…' : disabledHint,
                  suffixIconConstraints: const BoxConstraints(minHeight: 54),
                  suffixIcon: Padding(
                    padding: const EdgeInsetsDirectional.only(end: 5),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        if (voiceVisible) ...[
                          IconButton(
                            key: const Key('voice-capture-action'),
                            tooltip:
                                voiceRecording
                                    ? '停止錄音並轉寫'
                                    : voiceProcessing
                                    ? '語音正在清理或轉寫'
                                    : voiceReady
                                    ? '開始語音輸入 · $voiceSummary'
                                    : '語音模型尚未就緒，請到設定檢查',
                            constraints: const BoxConstraints.tightFor(
                              width: 44,
                              height: 44,
                            ),
                            onPressed:
                                voiceRecording
                                    ? onVoiceAction
                                    : canStartVoice
                                    ? onVoiceAction
                                    : null,
                            icon:
                                voiceProcessing && !voiceRecording
                                    ? const SizedBox.square(
                                      dimension: 18,
                                      child: CircularProgressIndicator(
                                        strokeWidth: 2,
                                      ),
                                    )
                                    : Icon(
                                      voiceRecording
                                          ? Icons.stop_rounded
                                          : Icons.mic_none_rounded,
                                    ),
                          ),
                          if (voiceBusy)
                            IconButton(
                              key: const Key('voice-capture-cancel'),
                              tooltip: '取消語音輸入並清除錄音',
                              constraints: const BoxConstraints.tightFor(
                                width: 44,
                                height: 44,
                              ),
                              onPressed: onVoiceCancel,
                              icon: const Icon(Icons.close_rounded),
                            ),
                        ],
                        ValueListenableBuilder<TextEditingValue>(
                          valueListenable: controller,
                          builder: (context, value, _) {
                            final canSend =
                                sendReady && value.text.trim().isNotEmpty;
                            return IconButton.filled(
                              key: const Key('send-message'),
                              tooltip: '送出',
                              constraints: const BoxConstraints.tightFor(
                                width: 44,
                                height: 44,
                              ),
                              onPressed: canSend ? onSend : null,
                              style: IconButton.styleFrom(
                                backgroundColor: colors.primary,
                                foregroundColor: colors.onPrimary,
                                disabledBackgroundColor: colors.onSurface
                                    .withValues(alpha: 0.12),
                                disabledForegroundColor: colors.onSurface
                                    .withValues(alpha: 0.38),
                              ),
                              icon:
                                  sending
                                      ? const SizedBox.square(
                                        dimension: 18,
                                        child: CircularProgressIndicator(
                                          strokeWidth: 2,
                                        ),
                                      )
                                      : const Icon(Icons.arrow_upward_rounded),
                            );
                          },
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
