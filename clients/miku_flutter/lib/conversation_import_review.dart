part of 'conversation_screen.dart';

enum _ImportDestination { currentSession, newSession }

class _ImportDecision {
  const _ImportDecision({required this.text, required this.destination});

  final String text;
  final _ImportDestination destination;
}

extension _ConversationImports on _ConversationScreenState {
  void _enqueueImport(SharedContent content) {
    final eventId = content.eventId;
    if (eventId != null) {
      if (_recentImportEventIds.contains(eventId)) return;
      _recentImportEventIds.add(eventId);
      if (_recentImportEventIds.length > 64) {
        _recentImportEventIds.removeAt(0);
      }
      if (content.source == SharedContentSource.quickCapture) {
        final active = _activeImport;
        if (active?.value.source == SharedContentSource.quickCapture) {
          active!.value = content;
          return;
        }
        _pendingImports.removeWhere(
          (pending) => pending.source == SharedContentSource.quickCapture,
        );
      }
    }
    _pendingImports.add(content);
    if (_initialConnectionComplete) unawaited(_drainImports());
  }

  Future<void> _drainImports() async {
    if (_processingImports || !_initialConnectionComplete) return;
    _processingImports = true;
    try {
      while (mounted && _pendingImports.isNotEmpty) {
        final content = _pendingImports.removeAt(0);
        await _reviewImport(content);
      }
    } finally {
      _processingImports = false;
    }
  }

  Future<void> _reviewImport(SharedContent content) async {
    final contentListenable = ValueNotifier(content);
    _activeImport = contentListenable;
    late final _ImportDecision? decision;
    try {
      decision = await showModalBottomSheet<_ImportDecision>(
        context: context,
        useSafeArea: true,
        isScrollControlled: true,
        showDragHandle: true,
        builder:
            (context) => _ImportReviewSheet(
              contentListenable: contentListenable,
              currentSessionAvailable:
                  _session != null && _presence != _PresenceState.ended,
            ),
      );
    } finally {
      if (identical(_activeImport, contentListenable)) _activeImport = null;
      contentListenable.dispose();
    }
    if (decision == null || !mounted) return;
    if (decision.destination == _ImportDestination.newSession) {
      await _connect(createNew: true);
      if (!mounted) return;
    }
    await _sendContent(decision.text, preserveComposerDraft: true);
  }
}

class _ImportReviewSheet extends StatefulWidget {
  const _ImportReviewSheet({
    required this.contentListenable,
    required this.currentSessionAvailable,
  });

  final ValueListenable<SharedContent> contentListenable;
  final bool currentSessionAvailable;

  @override
  State<_ImportReviewSheet> createState() => _ImportReviewSheetState();
}

class _ImportReviewSheetState extends State<_ImportReviewSheet> {
  late final TextEditingController _controller;
  late SharedContent _content;
  _ImportDestination? _destination;

  @override
  void initState() {
    super.initState();
    _content = widget.contentListenable.value;
    _controller = TextEditingController(text: _content.text);
    widget.contentListenable.addListener(_contentChanged);
  }

  void _contentChanged() {
    _content = widget.contentListenable.value;
    _controller.value = TextEditingValue(
      text: _content.text,
      selection: TextSelection.collapsed(offset: _content.text.length),
    );
    setState(() => _destination = null);
  }

  @override
  void dispose() {
    widget.contentListenable.removeListener(_contentChanged);
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final source = _content.source;
    final isVoice = source == SharedContentSource.voice;
    final canSend = _controller.text.trim().isNotEmpty && _destination != null;
    return FractionallySizedBox(
      key: const Key('import-review-sheet'),
      heightFactor: 0.88,
      child: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 680),
          child: Padding(
            padding: EdgeInsets.fromLTRB(
              20,
              4,
              20,
              20 + MediaQuery.viewInsetsOf(context).bottom,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    Icon(_importIcon(source), color: palette.miku),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(
                        _importTitle(source),
                        style: Theme.of(context).textTheme.titleLarge?.copyWith(
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    IconButton(
                      tooltip: '取消匯入',
                      onPressed: () => Navigator.of(context).pop(),
                      icon: const Icon(Icons.close_rounded),
                    ),
                  ],
                ),
                Text(
                  '先檢查、編輯並選擇目的地；TempestMiku 不會自動送出。',
                  style: Theme.of(
                    context,
                  ).textTheme.bodySmall?.copyWith(color: palette.muted),
                ),
                if (_content.truncated) ...[
                  const SizedBox(height: 10),
                  const _ImportWarning(
                    key: Key('import-truncated-warning'),
                    text: '來源內容超過安全上限，以下只保留可送出的部分。',
                  ),
                ],
                if (isVoice && _content.voiceQualityIssue != null) ...[
                  const SizedBox(height: 10),
                  _ImportWarning(
                    key: const Key('voice-quality-warning'),
                    text: _voiceQualityLabel(_content.voiceQualityIssue!),
                  ),
                ],
                if (isVoice && _content.voiceDiagnostics != null) ...[
                  const SizedBox(height: 10),
                  Text(
                    _voiceDiagnosticsLabel(_content.voiceDiagnostics!),
                    key: const Key('voice-diagnostics'),
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
                ],
                if (isVoice) ...[
                  const SizedBox(height: 8),
                  Text(
                    _voiceProvenanceLabel(
                      _content.voiceTranscriptProvenance ??
                          VoiceTranscriptProvenance.local,
                    ),
                    key: const Key('voice-transcript-provenance'),
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
                  if (_content.voiceBuildFingerprint case final build?) ...[
                    const SizedBox(height: 4),
                    Text(
                      'App ${build.versionName}+${build.versionCode} · '
                      '${build.buildType} · APK ${build.apkSha256.substring(0, 16)}…',
                      key: const Key('voice-build-fingerprint'),
                      style: Theme.of(
                        context,
                      ).textTheme.bodySmall?.copyWith(color: palette.muted),
                    ),
                  ],
                ],
                const SizedBox(height: 14),
                Expanded(
                  child: TextField(
                    key: const Key('import-review-editor'),
                    controller: _controller,
                    minLines: null,
                    maxLines: null,
                    expands: true,
                    maxLength: maxSharedTextLength,
                    textAlignVertical: TextAlignVertical.top,
                    decoration: const InputDecoration(
                      labelText: '送出前編輯',
                      alignLabelWithHint: true,
                      border: OutlineInputBorder(),
                    ),
                    onChanged: (_) => setState(() {}),
                  ),
                ),
                const SizedBox(height: 12),
                Text('送到', style: Theme.of(context).textTheme.labelLarge),
                const SizedBox(height: 8),
                SegmentedButton<_ImportDestination>(
                  key: const Key('import-destination'),
                  segments: [
                    ButtonSegment(
                      value: _ImportDestination.currentSession,
                      enabled: widget.currentSessionAvailable,
                      icon: const Icon(Icons.chat_bubble_outline, size: 18),
                      label: const Text('目前對話'),
                    ),
                    const ButtonSegment(
                      value: _ImportDestination.newSession,
                      icon: Icon(Icons.add_comment_outlined, size: 18),
                      label: Text('新對話'),
                    ),
                  ],
                  selected: {if (_destination != null) _destination!},
                  emptySelectionAllowed: true,
                  showSelectedIcon: false,
                  onSelectionChanged:
                      (selection) =>
                          setState(() => _destination = selection.first),
                ),
                const SizedBox(height: 14),
                FilledButton.icon(
                  key: const Key('send-import'),
                  onPressed:
                      canSend
                          ? () => Navigator.of(context).pop(
                            _ImportDecision(
                              text: _controller.text.trim(),
                              destination: _destination!,
                            ),
                          )
                          : null,
                  icon: const Icon(Icons.send_rounded),
                  label: const Text('確認並送給 Miku'),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _ImportWarning extends StatelessWidget {
  const _ImportWarning({required this.text, super.key});

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: palette.warm.withValues(alpha: 0.10),
        border: Border.all(color: palette.warm.withValues(alpha: 0.35)),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          Icon(Icons.warning_amber_rounded, color: palette.warm),
          const SizedBox(width: 9),
          Expanded(child: Text(text)),
        ],
      ),
    );
  }
}

String _importTitle(SharedContentSource source) => switch (source) {
  SharedContentSource.share => '分享給 Miku',
  SharedContentSource.selection => '詢問這段文字',
  SharedContentSource.quickCapture => '快速記下',
  SharedContentSource.voice => '語音轉寫草稿',
};

IconData _importIcon(SharedContentSource source) => switch (source) {
  SharedContentSource.share => Icons.share_outlined,
  SharedContentSource.selection => Icons.text_fields_rounded,
  SharedContentSource.quickCapture => Icons.edit_note_rounded,
  SharedContentSource.voice => Icons.graphic_eq_rounded,
};

String _voiceQualityLabel(VoiceCaptureQualityIssue issue) => switch (issue) {
  VoiceCaptureQualityIssue.tooShort => '錄音太短，請確認轉寫內容後再送出。',
  VoiceCaptureQualityIssue.tooQuiet => '錄音音量偏低，請確認轉寫內容後再送出。',
  VoiceCaptureQualityIssue.clipped => '錄音可能爆音，請確認轉寫內容後再送出。',
};

String _voiceDiagnosticsLabel(VoiceCaptureDiagnostics value) =>
    '錄音 ${(value.duration.inMilliseconds / 1000).toStringAsFixed(1)} 秒 · '
    'RMS ${value.rmsDbfs.toStringAsFixed(1)} dBFS · '
    'Peak ${value.peakDbfs.toStringAsFixed(1)} dBFS';

String _voiceProvenanceLabel(VoiceTranscriptProvenance value) =>
    switch (value) {
      VoiceTranscriptProvenance.local => '本機裝置端辨識 · 原始音訊已清除',
      VoiceTranscriptProvenance.selfHosted =>
        '固定家用自架服務辨識 · 音訊曾經配對 Server 傳送 · 無 fallback',
    };
