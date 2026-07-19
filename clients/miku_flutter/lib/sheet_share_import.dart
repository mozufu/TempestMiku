part of 'main.dart';

enum _ShareDestination { currentSession, newSession }

class _ShareImportDecision {
  const _ShareImportDecision({required this.text, required this.destination});

  final String text;
  final _ShareDestination destination;
}

class _ShareImportSheet extends StatefulWidget {
  const _ShareImportSheet({
    required this.contentListenable,
    required this.currentSessionAvailable,
    required this.tok,
    required this.copy,
  });

  final ValueListenable<SharedContent> contentListenable;
  final bool currentSessionAvailable;
  final _Tok tok;
  final _UiCopy copy;

  @override
  State<_ShareImportSheet> createState() => _ShareImportSheetState();
}

class _ShareImportSheetState extends State<_ShareImportSheet> {
  late final TextEditingController _controller;
  late SharedContent _content;
  _ShareDestination? _destination;

  @override
  void initState() {
    super.initState();
    _content = widget.contentListenable.value;
    _controller = TextEditingController(text: _content.text);
    widget.contentListenable.addListener(_handleContentChanged);
  }

  @override
  void didUpdateWidget(covariant _ShareImportSheet oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (identical(oldWidget.contentListenable, widget.contentListenable)) {
      return;
    }
    oldWidget.contentListenable.removeListener(_handleContentChanged);
    _content = widget.contentListenable.value;
    widget.contentListenable.addListener(_handleContentChanged);
    _replaceEditorContent();
  }

  void _handleContentChanged() {
    _content = widget.contentListenable.value;
    _replaceEditorContent();
  }

  void _replaceEditorContent() {
    _controller.value = TextEditingValue(
      text: _content.text,
      selection: TextSelection.collapsed(offset: _content.text.length),
    );
    setState(() => _destination = null);
  }

  @override
  void dispose() {
    widget.contentListenable.removeListener(_handleContentChanged);
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final copy = widget.copy;
    final isSelection = _content.source == SharedContentSource.selection;
    final isQuickCapture = _content.source == SharedContentSource.quickCapture;
    final isVoice = _content.source == SharedContentSource.voice;
    final voiceDiagnostics = isVoice ? _content.voiceDiagnostics : null;
    final voiceBuildFingerprint =
        isVoice ? _content.voiceBuildFingerprint : null;
    final voiceTranscriptProvenance =
        isVoice
            ? _content.voiceTranscriptProvenance ??
                VoiceTranscriptProvenance.local
            : VoiceTranscriptProvenance.local;
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
                        : isVoice
                        ? Icons.graphic_eq_rounded
                        : isQuickCapture
                        ? Icons.edit_note_rounded
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
                            : isVoice
                            ? copy.voiceCapture
                            : isQuickCapture
                            ? copy.quickCapture
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
            if (_content.truncated) ...[
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
                        isQuickCapture
                            ? copy.quickCaptureTruncated
                            : copy.shareTruncated,
                        style: TextStyle(color: tok.text, fontSize: 12.5),
                      ),
                    ),
                  ],
                ),
              ),
            ],
            if (isVoice && _content.voiceQualityIssue != null) ...[
              const SizedBox(height: 14),
              Container(
                key: const ValueKey('voiceCaptureQualityWarning'),
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
                        copy.voiceCaptureQualityWarning(
                          _content.voiceQualityIssue!,
                        ),
                        style: TextStyle(color: tok.text, fontSize: 12.5),
                      ),
                    ),
                  ],
                ),
              ),
            ],
            if (voiceDiagnostics != null) ...[
              const SizedBox(height: 14),
              Container(
                key: const ValueKey('voiceCaptureDiagnostics'),
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: tok.raised,
                  border: Border.all(color: tok.border),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Icon(
                          Icons.monitor_heart_outlined,
                          color: tok.accentSoft,
                          size: 19,
                        ),
                        const SizedBox(width: 9),
                        Text(
                          copy.voiceCaptureDiagnosticsTitle,
                          style: TextStyle(
                            color: tok.text,
                            fontSize: 12.5,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 8),
                    SelectableText(
                      copy.voiceCaptureDiagnosticsSummary(voiceDiagnostics),
                      key: const ValueKey('voiceCaptureDiagnosticsSummary'),
                      style: TextStyle(
                        color: tok.text,
                        fontSize: 12,
                        height: 1.45,
                      ),
                    ),
                    if (_content.eventId != null) ...[
                      const SizedBox(height: 6),
                      SelectableText(
                        copy.voiceCaptureId(_content.eventId!),
                        key: const ValueKey('voiceCaptureId'),
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 11.5,
                          height: 1.4,
                        ),
                      ),
                    ],
                    const SizedBox(height: 6),
                    Text(
                      copy.voiceCaptureDiagnosticsPrivacy(
                        voiceTranscriptProvenance,
                      ),
                      style: TextStyle(color: tok.muted, fontSize: 11.5),
                    ),
                  ],
                ),
              ),
            ],
            if (isVoice) ...[
              const SizedBox(height: 14),
              Container(
                key: const ValueKey('voiceBuildFingerprint'),
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: tok.raised,
                  border: Border.all(color: tok.border),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Icon(
                          Icons.fingerprint_rounded,
                          color: tok.accentSoft,
                          size: 19,
                        ),
                        const SizedBox(width: 9),
                        Text(
                          copy.voiceBuildFingerprintTitle,
                          style: TextStyle(
                            color: tok.text,
                            fontSize: 12.5,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 8),
                    SelectableText(
                      voiceBuildFingerprint == null
                          ? copy.voiceBuildFingerprintUnavailable
                          : copy.voiceBuildFingerprintSummary(
                            voiceBuildFingerprint,
                          ),
                      key: const ValueKey('voiceBuildFingerprintSummary'),
                      style: TextStyle(
                        color:
                            voiceBuildFingerprint == null
                                ? tok.muted
                                : tok.text,
                        fontSize: 12,
                        height: 1.45,
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
                labelText:
                    isSelection
                        ? copy.selectedText
                        : isVoice
                        ? copy.transcriptDraft
                        : isQuickCapture
                        ? copy.captureDraft
                        : copy.sharedContent,
                helperText:
                    isSelection
                        ? copy.selectedFromAndroid
                        : isVoice
                        ? voiceTranscriptProvenance ==
                                VoiceTranscriptProvenance.selfHosted
                            ? copy.voiceCapturedSelfHosted
                            : copy.voiceCapturedOnDevice
                        : isQuickCapture
                        ? copy.quickCaptureFromAndroid
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
