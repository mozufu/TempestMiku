part of 'main.dart';

class _ActivityRow extends StatelessWidget {
  const _ActivityRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.item,
    required this.onOpenResource,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final _ActivityItem item;
  final void Function(String) onOpenResource;

  @override
  Widget build(BuildContext context) {
    final stateColor = _stateColor(item.state);
    final detail = _trimActivityDetail(item.detail);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 22,
          height: 22,
          decoration: BoxDecoration(
            color: stateColor.withOpacity(0.12),
            border: Border.all(color: stateColor.withOpacity(0.38)),
            borderRadius: BorderRadius.circular(7),
          ),
          child: Icon(item.icon, size: 13, color: stateColor),
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                item.title,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 12.2,
                  fontWeight: FontWeight.w800,
                ),
              ),
              if (detail.isNotEmpty) ...[
                const SizedBox(height: 3),
                Text(
                  detail,
                  maxLines: item.monospace ? 5 : 3,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: item.monospace ? 11 : 11.5,
                    fontWeight: FontWeight.w600,
                    height: 1.35,
                    fontFamily: item.monospace ? 'monospace' : null,
                  ),
                ),
              ],
              if (item.resourceUris.isNotEmpty) ...[
                const SizedBox(height: 6),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: item.resourceUris
                      .map(
                        (uri) => Semantics(
                          button: true,
                          label: copy.openActivityResource(uri),
                          child: Material(
                            color: Colors.transparent,
                            child: InkWell(
                              key: ValueKey('activity-resource:$uri'),
                              onTap: () => onOpenResource(uri),
                              borderRadius: BorderRadius.circular(8),
                              focusColor: tok.focus.withOpacity(0.18),
                              child: Container(
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 8,
                                  vertical: 6,
                                ),
                                decoration: BoxDecoration(
                                  color: tok.surface,
                                  border: Border.all(color: tok.border),
                                  borderRadius: BorderRadius.circular(8),
                                ),
                                child: Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    Icon(
                                      Icons.insert_drive_file_outlined,
                                      size: 12,
                                      color: accent,
                                    ),
                                    const SizedBox(width: 5),
                                    ConstrainedBox(
                                      constraints:
                                          const BoxConstraints(maxWidth: 230),
                                      child: Text(
                                        uri,
                                        maxLines: 1,
                                        overflow: TextOverflow.ellipsis,
                                        style: TextStyle(
                                          color: accent,
                                          fontSize: 10.8,
                                          fontWeight: FontWeight.w800,
                                          fontFamily: 'monospace',
                                        ),
                                      ),
                                    ),
                                    const SizedBox(width: 4),
                                    Icon(
                                      Icons.open_in_new,
                                      size: 10,
                                      color: tok.muted,
                                    ),
                                  ],
                                ),
                              ),
                            ),
                          ),
                        ),
                      )
                      .toList(),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Color _stateColor(_ActivityState state) => switch (state) {
        _ActivityState.running => accent,
        _ActivityState.done => tok.success,
        _ActivityState.failed => tok.danger,
        _ActivityState.info => tok.muted,
      };
}

String _trimActivityDetail(String detail) {
  final cleaned = detail
      .trim()
      .split('\n')
      .map((line) => line.trimRight())
      .where((line) => line.trim().isNotEmpty)
      .join('\n');
  if (cleaned.length <= 520) return cleaned;
  return '${cleaned.substring(0, 517)}...';
}

class _ApprovalCard extends StatelessWidget {
  const _ApprovalCard({
    required this.tok,
    required this.copy,
    required this.approval,
    required this.accent,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final ApprovalPrompt approval;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: copy.pendingApprovalSemantics(approval.action),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('approval:${approval.action}'),
          onTap: onTap,
          borderRadius: BorderRadius.circular(13),
          focusColor: tok.focus.withOpacity(0.18),
          child: Container(
            padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
            decoration: BoxDecoration(
              color: tok.warning.withOpacity(0.11),
              border: Border.all(color: tok.warning.withOpacity(0.52)),
              borderRadius: BorderRadius.circular(13),
            ),
            child: Row(
              children: [
                Container(
                  width: 34,
                  height: 34,
                  decoration: BoxDecoration(
                    color: tok.warning,
                    borderRadius: BorderRadius.circular(9),
                  ),
                  child: Icon(
                    Icons.warning_amber_rounded,
                    color: _textOn(tok.warning),
                    size: 18,
                  ),
                ),
                const SizedBox(width: 11),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        copy.pendingApproval(approval.action),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 13,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        copy.tapForDetails,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 11.5,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ],
                  ),
                ),
                Icon(Icons.chevron_right, color: tok.muted, size: 18),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _MemoryProposalCard extends StatelessWidget {
  const _MemoryProposalCard({
    required this.tok,
    required this.copy,
    required this.proposal,
    required this.approval,
    required this.accent,
    required this.onApprove,
    required this.onDeny,
  });

  final _Tok tok;
  final _UiCopy copy;
  final MemoryWriteProposal proposal;
  final ApprovalPrompt? approval;
  final Color accent;
  final VoidCallback? onApprove;
  final VoidCallback? onDeny;

  @override
  Widget build(BuildContext context) {
    final hasApproval = approval != null;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
      decoration: BoxDecoration(
        color: tok.surface,
        border: Border.all(color: accent.withOpacity(0.46)),
        borderRadius: BorderRadius.circular(13),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              color: accent,
              borderRadius: BorderRadius.circular(9),
            ),
            child: Icon(Icons.memory, color: _textOn(accent), size: 18),
          ),
          const SizedBox(width: 11),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        copy.memoryProposal,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 12.8,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    Container(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 7,
                        vertical: 2,
                      ),
                      decoration: BoxDecoration(
                        color: accent.withOpacity(0.1),
                        borderRadius: BorderRadius.circular(999),
                      ),
                      child: Text(
                        hasApproval ? copy.pending : copy.syncing,
                        style: TextStyle(
                          color: accent,
                          fontSize: 10.5,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 5),
                Text(
                  proposal.displayText,
                  maxLines: 3,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    height: 1.42,
                  ),
                ),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.label_outline,
                      text: proposal.kindLabel,
                    ),
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.folder_outlined,
                      text: copy.scopeChip(proposal.scopeLabel),
                    ),
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.history,
                      text: copy.provenanceChip(proposal.provenanceText),
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                Row(
                  children: [
                    _ProposalActionButton(
                      tok: tok,
                      icon: Icons.close,
                      label: copy.deny,
                      disabledLabel: copy.waitingForApproval,
                      waitingLabel: copy.waiting,
                      onTap: onDeny,
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: _ProposalActionButton(
                        tok: tok,
                        icon: Icons.check,
                        label: copy.saveMemory,
                        disabledLabel: copy.waitingForApproval,
                        waitingLabel: copy.waiting,
                        accent: accent,
                        onTap: onApprove,
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _ProposalChip extends StatelessWidget {
  const _ProposalChip({
    required this.tok,
    required this.icon,
    required this.text,
  });

  final _Tok tok;
  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 12, color: tok.muted),
          const SizedBox(width: 5),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 210),
            child: Text(
              text,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: tok.muted,
                fontSize: 10.8,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ProposalActionButton extends StatelessWidget {
  const _ProposalActionButton({
    required this.tok,
    required this.icon,
    required this.label,
    required this.disabledLabel,
    required this.waitingLabel,
    required this.onTap,
    this.accent,
  });

  final _Tok tok;
  final IconData icon;
  final String label;
  final String disabledLabel;
  final String waitingLabel;
  final VoidCallback? onTap;
  final Color? accent;

  @override
  Widget build(BuildContext context) {
    final active = onTap != null;
    final bg = accent ?? tok.bg;
    final fg = accent == null ? tok.text : _textOn(accent!);
    final border = accent ?? tok.border;
    return Opacity(
      opacity: active ? 1 : 0.54,
      child: Semantics(
        button: true,
        enabled: active,
        label: active ? label : disabledLabel,
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(10),
            focusColor: tok.focus.withOpacity(0.18),
            child: Container(
              height: 38,
              padding: const EdgeInsets.symmetric(horizontal: 10),
              decoration: BoxDecoration(
                color: bg,
                border: Border.all(color: border),
                borderRadius: BorderRadius.circular(10),
              ),
              child: Center(
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(icon, color: fg, size: 15),
                    const SizedBox(width: 6),
                    Flexible(
                      child: Text(
                        active ? label : waitingLabel,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: fg,
                          fontSize: 12.3,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
