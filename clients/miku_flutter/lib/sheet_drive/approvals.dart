part of '../main.dart';

class _DriveApprovalRow extends StatelessWidget {
  const _DriveApprovalRow({
    required this.tok,
    required this.copy,
    required this.approval,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final ApprovalPrompt approval;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: copy.pendingApprovalSemantics(approval.action),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-approval:${approval.approvalId}'),
          onTap: onTap,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: _DrivePendingShell(
            tok: tok,
            icon: Icons.warning_amber_rounded,
            title: approval.action,
            detail: copy.tapForDetails,
          ),
        ),
      ),
    );
  }
}

class _DrivePendingApprovalRow extends StatelessWidget {
  const _DrivePendingApprovalRow({required this.tok, required this.approval});

  final _Tok tok;
  final DrivePendingApproval approval;

  @override
  Widget build(BuildContext context) {
    return _DrivePendingShell(
      tok: tok,
      icon: Icons.pending_actions,
      title: approval.action,
      detail: approval.preview ?? approval.approvalId,
    );
  }
}

class _DrivePendingShell extends StatelessWidget {
  const _DrivePendingShell({
    required this.tok,
    required this.icon,
    required this.title,
    required this.detail,
  });

  final _Tok tok;
  final IconData icon;
  final String title;
  final String detail;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
      decoration: BoxDecoration(
        color: tok.warning.withValues(alpha: 0.1),
        border: Border.all(color: tok.warning.withValues(alpha: 0.48)),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          Icon(icon, color: tok.warning, size: 17),
          const SizedBox(width: 9),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12.5,
                    fontWeight: FontWeight.w800,
                  ),
                ),
                if (detail.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Text(
                    detail,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11.2,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}
