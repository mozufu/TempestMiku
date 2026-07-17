part of '../main.dart';

class _DriveProposalRow extends StatelessWidget {
  const _DriveProposalRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.proposal,
    required this.onOpen,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final DriveOrganizerProposal proposal;
  final VoidCallback? onOpen;

  @override
  Widget build(BuildContext context) {
    final confidence = proposal.confidence;
    final row = Container(
      padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              color: accent.withValues(alpha: 0.12),
              border: Border.all(color: accent.withValues(alpha: 0.38)),
              borderRadius: BorderRadius.circular(9),
            ),
            child: Icon(Icons.rule_folder_outlined, color: accent, size: 16),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        proposal.displayTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 12.7,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    Text(
                      proposal.status,
                      style: TextStyle(
                        color: accent,
                        fontSize: 10.5,
                        fontWeight: FontWeight.w900,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 3),
                Text(
                  proposal.displayPath,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11.3,
                    fontWeight: FontWeight.w600,
                    height: 1.35,
                  ),
                ),
                if (proposal.previewSnippet?.isNotEmpty == true) ...[
                  const SizedBox(height: 4),
                  Text(
                    proposal.previewSnippet!,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11,
                      fontWeight: FontWeight.w500,
                      height: 1.35,
                    ),
                  ),
                ],
                if (confidence != null) ...[
                  const SizedBox(height: 7),
                  _HistoryChip(
                    tok: tok,
                    icon: Icons.query_stats,
                    text: copy.driveConfidence(confidence),
                  ),
                ],
              ],
            ),
          ),
          if (onOpen != null) ...[
            const SizedBox(width: 6),
            Icon(Icons.chevron_right, color: tok.muted, size: 17),
          ],
        ],
      ),
    );
    if (onOpen == null) return row;
    return Semantics(
      button: true,
      label: proposal.sourceUri!,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onOpen,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: row,
        ),
      ),
    );
  }
}
