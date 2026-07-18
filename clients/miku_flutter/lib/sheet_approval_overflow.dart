part of 'main.dart';

class _ApprovalSheet extends StatefulWidget {
  const _ApprovalSheet({
    required this.approval,
    required this.tok,
    required this.copy,
    required this.accent,
    required this.onOption,
    required this.onApprove,
    required this.onDeny,
  });

  final ApprovalPrompt approval;
  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final void Function(ApprovalOption option) onOption;
  final VoidCallback onApprove, onDeny;

  @override
  State<_ApprovalSheet> createState() => _ApprovalSheetState();
}

class _ApprovalSheetState extends State<_ApprovalSheet> {
  late final int _initialSecs;
  late int _secs;
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _initialSecs = math.max(
      1,
      ((widget.approval.timeoutMs ?? 60000) / 1000).ceil(),
    );
    _secs = _initialSecs;
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      setState(() => _secs--);
      if (_secs <= 0) {
        _timer?.cancel();
      }
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final copy = widget.copy;
    final accent = tok.warning;
    final approveColor = widget.accent;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 9, 16, 18),
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 38,
              height: 5,
              decoration: BoxDecoration(
                color: tok.border,
                borderRadius: BorderRadius.circular(999),
              ),
            ),
            const SizedBox(height: 14),
            Row(
              children: [
                Container(
                  width: 40,
                  height: 40,
                  decoration: BoxDecoration(
                    color: accent,
                    borderRadius: BorderRadius.circular(11),
                  ),
                  child: Icon(
                    Icons.warning_amber_rounded,
                    color: _textOn(accent),
                    size: 21,
                  ),
                ),
                const SizedBox(width: 11),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        copy.approvalNeeded,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 17,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                      const SizedBox(height: 1),
                      Text(
                        copy.approvalHelper,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 12,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
            const SizedBox(height: 13),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(13),
              decoration: BoxDecoration(
                color: tok.bg,
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(13),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    widget.approval.action,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 12,
                      fontWeight: FontWeight.w700,
                      fontFamily: 'monospace',
                      height: 1.5,
                    ),
                  ),
                  if (widget.approval.scope.isNotEmpty) ...[
                    const SizedBox(height: 9),
                    Wrap(
                      spacing: 7,
                      runSpacing: 6,
                      children:
                          widget.approval.scope.entries.map((e) {
                            return Container(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 9,
                                vertical: 3,
                              ),
                              decoration: BoxDecoration(
                                color: tok.surface,
                                borderRadius: BorderRadius.circular(999),
                              ),
                              child: Text(
                                '${e.key}: ${e.value}',
                                style: TextStyle(
                                  color: tok.muted,
                                  fontSize: 11,
                                  fontWeight: FontWeight.w700,
                                ),
                              ),
                            );
                          }).toList(),
                    ),
                  ],
                ],
              ),
            ),
            const SizedBox(height: 13),
            Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                Text(
                  copy.autoDeny,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11.5,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                Text(
                  '${_secs}s',
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 11,
                    fontWeight: FontWeight.w800,
                    fontFamily: 'monospace',
                  ),
                ),
              ],
            ),
            const SizedBox(height: 6),
            ClipRRect(
              borderRadius: BorderRadius.circular(999),
              child: LinearProgressIndicator(
                value: _secs / _initialSecs,
                backgroundColor: tok.border.withValues(alpha: 0.6),
                valueColor: AlwaysStoppedAnimation<Color>(accent),
                minHeight: 5,
              ),
            ),
            const SizedBox(height: 14),
            if (widget.approval.options.isEmpty)
              Row(
                children: [
                  Expanded(
                    child: Semantics(
                      button: true,
                      label: copy.deny,
                      child: Material(
                        color: Colors.transparent,
                        child: InkWell(
                          onTap: widget.onDeny,
                          borderRadius: BorderRadius.circular(13),
                          focusColor: tok.focus.withValues(alpha: 0.18),
                          child: Container(
                            height: 48,
                            decoration: BoxDecoration(
                              color: tok.bg,
                              border: Border.all(color: tok.danger),
                              borderRadius: BorderRadius.circular(13),
                            ),
                            child: Center(
                              child: Text(
                                copy.deny,
                                style: TextStyle(
                                  color: tok.text,
                                  fontSize: 14.5,
                                  fontWeight: FontWeight.w700,
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    flex: 3,
                    child: Semantics(
                      button: true,
                      label: copy.approveOnce,
                      child: Material(
                        color: Colors.transparent,
                        child: InkWell(
                          onTap: widget.onApprove,
                          borderRadius: BorderRadius.circular(13),
                          focusColor: tok.focus.withValues(alpha: 0.18),
                          child: Container(
                            height: 48,
                            decoration: BoxDecoration(
                              color: approveColor,
                              borderRadius: BorderRadius.circular(13),
                            ),
                            child: Center(
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  Icon(
                                    Icons.check,
                                    color: _textOn(approveColor),
                                    size: 17,
                                  ),
                                  const SizedBox(width: 7),
                                  Text(
                                    copy.approveOnce,
                                    style: TextStyle(
                                      color: _textOn(approveColor),
                                      fontSize: 14.5,
                                      fontWeight: FontWeight.w800,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              )
            else
              Column(
                children:
                    widget.approval.options.map((option) {
                      final isReject =
                          option.kind.startsWith('reject') ||
                          option.kind.startsWith('deny');
                      final buttonColor = isReject ? tok.bg : approveColor;
                      final textColor =
                          isReject ? tok.text : _textOn(approveColor);
                      final rawLabel =
                          option.name.isEmpty ? option.optionId : option.name;
                      final label =
                          isReject
                              ? copy.deny
                              : rawLabel.toLowerCase().contains('allow')
                              ? copy.approveOnce
                              : rawLabel;
                      return Padding(
                        padding: const EdgeInsets.only(bottom: 8),
                        child: Semantics(
                          button: true,
                          label: label,
                          child: Material(
                            color: Colors.transparent,
                            child: InkWell(
                              onTap: () => widget.onOption(option),
                              borderRadius: BorderRadius.circular(13),
                              focusColor: tok.focus.withValues(alpha: 0.18),
                              child: Container(
                                width: double.infinity,
                                constraints: const BoxConstraints(
                                  minHeight: 48,
                                ),
                                decoration: BoxDecoration(
                                  color: buttonColor,
                                  border: Border.all(
                                    color: isReject ? tok.danger : approveColor,
                                  ),
                                  borderRadius: BorderRadius.circular(13),
                                ),
                                child: Center(
                                  child: Text(
                                    label,
                                    style: TextStyle(
                                      color: textColor,
                                      fontSize: 14,
                                      fontWeight: FontWeight.w800,
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          ),
                        ),
                      );
                    }).toList(),
              ),
          ],
        ),
      ),
    );
  }
}
