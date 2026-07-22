part of 'conversation_screen.dart';

class _ApprovalCard extends StatefulWidget {
  const _ApprovalCard({required this.item, required this.onSelect});

  final _ApprovalItem item;
  final ValueChanged<ApprovalOption> onSelect;

  @override
  State<_ApprovalCard> createState() => _ApprovalCardState();
}

class _ApprovalCardState extends State<_ApprovalCard> {
  Timer? _countdown;
  DateTime? _deadline;

  @override
  void initState() {
    super.initState();
    final timeoutMs = widget.item.prompt.timeoutMs;
    if (timeoutMs != null && widget.item.resolvedStatus == null) {
      _deadline = DateTime.now().add(Duration(milliseconds: timeoutMs));
      _countdown = Timer.periodic(const Duration(seconds: 1), (_) {
        if (!mounted) return;
        if (widget.item.resolvedStatus != null || _remainingSeconds() <= 0) {
          _stopCountdown();
        }
        setState(() {});
      });
    }
  }

  @override
  void dispose() {
    _countdown?.cancel();
    super.dispose();
  }

  void _stopCountdown() {
    _countdown?.cancel();
    _countdown = null;
  }

  int _remainingSeconds() {
    final deadline = _deadline;
    if (deadline == null) return 0;
    final remaining = deadline.difference(DateTime.now()).inSeconds;
    return remaining > 0 ? remaining : 0;
  }

  @override
  Widget build(BuildContext context) {
    final item = widget.item;
    final onSelect = widget.onSelect;
    final palette = _Palette.of(context);
    final resolved = item.resolvedStatus;
    final memoryProposal = MemoryWriteProposal.fromApproval(item.prompt);
    final evolutionProposal = EvolutionReviewProposal.fromEvent({
      ...item.prompt.scope,
      'status': 'pending',
    });
    final rollbackProposal = _rollbackReviewDetails(item.prompt.scope);
    final genericScope =
        memoryProposal == null &&
                evolutionProposal == null &&
                rollbackProposal == null
            ? _scopeLabel(item.prompt.scope)
            : null;
    return Semantics(
      liveRegion: true,
      container: true,
      label: '需要確認：${item.prompt.action}',
      child: Container(
        key: Key('approval-${item.prompt.approvalId}'),
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          color: palette.approvalSurface,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(color: palette.approvalOutline),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.shield_outlined, size: 18, color: palette.warm),
                const SizedBox(width: 8),
                Text('需要你的確認', style: Theme.of(context).textTheme.labelLarge),
              ],
            ),
            const SizedBox(height: 10),
            SelectableText(
              item.prompt.action,
              style: Theme.of(context).textTheme.bodyMedium,
            ),
            if (genericScope case final scope?) ...[
              const SizedBox(height: 5),
              Text(
                scope,
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(color: palette.muted),
              ),
            ],
            if (memoryProposal case final proposal?) ...[
              const SizedBox(height: 12),
              _MemoryProposalDetails(proposal: proposal),
            ] else if (evolutionProposal case final proposal?) ...[
              const SizedBox(height: 12),
              _EvolutionProposalDetails(proposal: proposal),
            ] else if (rollbackProposal case final proposal?) ...[
              const SizedBox(height: 12),
              _RollbackProposalDetails(details: proposal),
            ],
            if (resolved == null && _deadline != null) ...[
              const SizedBox(height: 8),
              Text(
                _remainingSeconds() > 0
                    ? '還有 ${_remainingSeconds()}s 可以決定 · 逾時將視為拒絕'
                    : '已逾時，視為拒絕',
                key: Key('approval-timeout-${item.prompt.approvalId}'),
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(color: palette.muted),
              ),
            ],
            if (item.error != null) ...[
              const SizedBox(height: 8),
              Text(
                item.error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            ],
            const SizedBox(height: 14),
            if (resolved != null)
              Text(
                _approvalResolutionLabel(resolved),
                key: const Key('approval-resolution'),
                style: Theme.of(context).textTheme.labelLarge?.copyWith(
                  color: resolved == 'approved' ? palette.miku : palette.muted,
                ),
              )
            else
              Wrap(
                spacing: 10,
                runSpacing: 8,
                children: [
                  for (final option in item.prompt.options)
                    _ApprovalButton(
                      option: option,
                      enabled: !item.resolving,
                      onPressed: () => onSelect(option),
                    ),
                  if (item.resolving)
                    const Padding(
                      padding: EdgeInsets.all(10),
                      child: SizedBox.square(
                        dimension: 17,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      ),
                    ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

class _MemoryProposalDetails extends StatelessWidget {
  const _MemoryProposalDetails({required this.proposal});

  final MemoryWriteProposal proposal;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      key: const Key('memory-proposal-details'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: palette.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              Chip(
                avatar: const Icon(Icons.psychology_outlined, size: 16),
                label: Text(proposal.kindLabel),
                visualDensity: VisualDensity.compact,
              ),
              Chip(
                label: Text(proposal.scopeLabel),
                visualDensity: VisualDensity.compact,
              ),
            ],
          ),
          const SizedBox(height: 8),
          SelectableText(proposal.displayText),
          const SizedBox(height: 8),
          Text(
            '來源：${proposal.provenanceText}',
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(color: palette.muted),
          ),
        ],
      ),
    );
  }
}

class _EvolutionProposalDetails extends StatelessWidget {
  const _EvolutionProposalDetails({required this.proposal});

  final EvolutionReviewProposal proposal;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final targetLabel = proposal.targetKind == 'persona' ? 'Persona' : 'Mode';
    return Container(
      key: const Key('evolution-proposal-details'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: palette.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.auto_fix_high_outlined, size: 17),
              const SizedBox(width: 7),
              Expanded(
                child: Text(
                  '$targetLabel · ${proposal.targetId}',
                  style: Theme.of(context).textTheme.labelLarge,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          SelectableText(proposal.preview),
          const SizedBox(height: 8),
          Text(
            proposal.applyEnabled
                ? '核准後會建立不可變版本並啟用。'
                : '核准後只保留為 review，不會自動啟用。',
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(color: palette.muted),
          ),
          if (proposal.isAutoCandidate && proposal.evidenceCount != null)
            Text(
              '跨對話候選 · ${proposal.evidenceCount} 筆證據',
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
        ],
      ),
    );
  }
}

class _ApprovalButton extends StatelessWidget {
  const _ApprovalButton({
    required this.option,
    required this.enabled,
    required this.onPressed,
  });

  final ApprovalOption option;
  final bool enabled;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final approve = _isApprovalApproveKind(option.kind);
    if (approve) {
      return FilledButton(
        key: Key('approval-option-${option.optionId}'),
        onPressed: enabled ? onPressed : null,
        child: Text(option.name),
      );
    }
    return OutlinedButton(
      key: Key('approval-option-${option.optionId}'),
      onPressed: enabled ? onPressed : null,
      child: Text(option.name),
    );
  }
}
