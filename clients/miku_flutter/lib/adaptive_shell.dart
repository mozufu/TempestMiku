part of 'main.dart';

enum _AppDestination { chat, sessions, drive }

extension on _AppDestination {
  IconData get icon => switch (this) {
    _AppDestination.chat => Icons.chat_bubble_outline_rounded,
    _AppDestination.sessions => Icons.forum_outlined,
    _AppDestination.drive => Icons.folder_outlined,
  };

  IconData get selectedIcon => switch (this) {
    _AppDestination.chat => Icons.chat_bubble_rounded,
    _AppDestination.sessions => Icons.forum_rounded,
    _AppDestination.drive => Icons.folder_rounded,
  };

  String label(_UiCopy copy) => switch (this) {
    _AppDestination.chat => copy.pick('Chat', '聊天'),
    _AppDestination.sessions => copy.sessions,
    _AppDestination.drive => copy.driveFeed,
  };
}

class _MikuBottomNavigation extends StatelessWidget {
  const _MikuBottomNavigation({
    required this.destination,
    required this.copy,
    required this.onSelected,
  });

  final _AppDestination destination;
  final _UiCopy copy;
  final ValueChanged<_AppDestination> onSelected;

  @override
  Widget build(BuildContext context) {
    return NavigationBar(
      height: 68,
      selectedIndex: destination.index,
      labelBehavior: NavigationDestinationLabelBehavior.alwaysShow,
      onDestinationSelected:
          (index) => onSelected(_AppDestination.values[index]),
      destinations: [
        for (final item in _AppDestination.values)
          NavigationDestination(
            icon: Icon(item.icon),
            selectedIcon: Icon(item.selectedIcon),
            label: item.label(copy),
            tooltip: item.label(copy),
          ),
      ],
    );
  }
}

class _MikuNavigationRail extends StatelessWidget {
  const _MikuNavigationRail({
    required this.destination,
    required this.copy,
    required this.onSelected,
    required this.brand,
    required this.onSettings,
  });

  final _AppDestination destination;
  final _UiCopy copy;
  final ValueChanged<_AppDestination> onSelected;
  final Widget brand;
  final VoidCallback onSettings;

  @override
  Widget build(BuildContext context) {
    return NavigationRail(
      selectedIndex: destination.index,
      labelType: NavigationRailLabelType.all,
      groupAlignment: -0.72,
      leading: Padding(
        padding: const EdgeInsets.only(top: 12, bottom: 22),
        child: brand,
      ),
      trailing: Expanded(
        child: Align(
          alignment: Alignment.bottomCenter,
          child: Padding(
            padding: const EdgeInsets.only(bottom: 16),
            child: IconButton.filledTonal(
              tooltip: copy.pick('Settings', '設定'),
              onPressed: onSettings,
              icon: const Icon(Icons.tune_rounded),
            ),
          ),
        ),
      ),
      onDestinationSelected:
          (index) => onSelected(_AppDestination.values[index]),
      destinations: [
        for (final item in _AppDestination.values)
          NavigationRailDestination(
            icon: Icon(item.icon),
            selectedIcon: Icon(item.selectedIcon),
            label: Text(item.label(copy)),
          ),
      ],
    );
  }
}

class _ApprovalAttentionBar extends StatelessWidget {
  const _ApprovalAttentionBar({
    required this.tok,
    required this.copy,
    required this.approvals,
    required this.onOpen,
  });

  final _Tok tok;
  final _UiCopy copy;
  final List<ApprovalPrompt> approvals;
  final ValueChanged<ApprovalPrompt> onOpen;

  @override
  Widget build(BuildContext context) {
    if (approvals.isEmpty) return const SizedBox.shrink();
    final count = approvals.length;
    final text = copy.pick(
      count == 1
          ? 'Miku needs your approval'
          : '$count approvals need attention',
      count == 1 ? 'Miku 需要你的核可' : '$count 個操作等待核可',
    );
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 4),
      child: Material(
        color: tok.warning.withValues(alpha: 0.13),
        shape: RoundedRectangleBorder(
          side: BorderSide(color: tok.warning.withValues(alpha: 0.5)),
          borderRadius: BorderRadius.circular(18),
        ),
        child: InkWell(
          borderRadius: BorderRadius.circular(18),
          onTap: () => onOpen(approvals.first),
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 52),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
              child: Row(
                children: [
                  Icon(Icons.shield_outlined, color: tok.warning, size: 21),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(
                          text,
                          style: TextStyle(
                            color: tok.text,
                            fontSize: 13.5,
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                        const SizedBox(height: 2),
                        Text(
                          approvals.first.action,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(color: tok.muted, fontSize: 12),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 8),
                  Icon(Icons.chevron_right_rounded, color: tok.muted),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _ConnectionBanner extends StatelessWidget {
  const _ConnectionBanner({
    required this.tok,
    required this.copy,
    required this.status,
    required this.onRetry,
    required this.onNewSession,
  });

  final _Tok tok;
  final _UiCopy copy;
  final String status;
  final VoidCallback onRetry;
  final VoidCallback onNewSession;

  @override
  Widget build(BuildContext context) {
    if (status == 'connected' ||
        status == 'streaming' ||
        status == 'complete') {
      return const SizedBox.shrink();
    }
    final ended = status == 'ended';
    final busy = status == 'connecting' || status == 'reconnecting';
    final color =
        ended
            ? tok.muted
            : busy
            ? tok.cool
            : tok.warning;
    return Semantics(
      liveRegion: true,
      child: Container(
        width: double.infinity,
        color: color.withValues(alpha: 0.1),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        child: Row(
          children: [
            if (busy)
              SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(strokeWidth: 2, color: color),
              )
            else
              Icon(
                ended ? Icons.check_circle_outline : Icons.cloud_off_outlined,
                size: 18,
                color: color,
              ),
            const SizedBox(width: 9),
            Expanded(
              child: Text(
                copy.statusLabel(status),
                style: TextStyle(
                  color: tok.text,
                  fontSize: 12.5,
                  fontWeight: FontWeight.w700,
                ),
              ),
            ),
            if (!busy)
              TextButton(
                onPressed: ended ? onNewSession : onRetry,
                child: Text(ended ? copy.newSession : copy.pick('Retry', '重試')),
              ),
          ],
        ),
      ),
    );
  }
}

class _PairingWelcome extends StatelessWidget {
  const _PairingWelcome({
    required this.tok,
    required this.copy,
    required this.brand,
    required this.onScan,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Widget brand;
  final VoidCallback onScan;

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 520),
            child: Container(
              padding: const EdgeInsets.fromLTRB(26, 30, 26, 26),
              decoration: BoxDecoration(
                color: tok.raised.withValues(alpha: 0.94),
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(30),
                boxShadow: [
                  BoxShadow(
                    color: tok.accent.withValues(alpha: 0.14),
                    blurRadius: 48,
                    offset: const Offset(0, 20),
                  ),
                ],
              ),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  brand,
                  const SizedBox(height: 22),
                  Text(
                    copy.pick('Bring Miku with you', '把 Miku 帶在身邊'),
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 27,
                      height: 1.15,
                      fontWeight: FontWeight.w900,
                    ),
                  ),
                  const SizedBox(height: 12),
                  Text(
                    copy.pick(
                      'Scan the one-time QR from your TempestMiku server. You will review the exact origin and device authority before anything is stored.',
                      '掃描 TempestMiku server 的一次性 QR。儲存任何憑證前，你會先確認完整來源與裝置權限。',
                    ),
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 14.5,
                      height: 1.55,
                    ),
                  ),
                  const SizedBox(height: 24),
                  SizedBox(
                    width: double.infinity,
                    height: 52,
                    child: FilledButton.icon(
                      onPressed: onScan,
                      icon: const Icon(Icons.qr_code_scanner_rounded),
                      label: Text(copy.pick('Scan pairing QR', '掃描配對 QR')),
                    ),
                  ),
                  const SizedBox(height: 14),
                  Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Icon(
                        Icons.lock_outline_rounded,
                        size: 16,
                        color: tok.cool,
                      ),
                      const SizedBox(width: 6),
                      Flexible(
                        child: Text(
                          copy.pick(
                            'Origin-bound credentials · no token in the QR',
                            '憑證綁定來源 · QR 不含登入 token',
                          ),
                          textAlign: TextAlign.center,
                          style: TextStyle(
                            color: tok.muted,
                            fontSize: 11.5,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _MikuContextPanel extends StatelessWidget {
  const _MikuContextPanel({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.projectStatus,
    required this.nextActions,
    required this.approvals,
    required this.onOpenApproval,
    required this.onPromote,
    required this.onRefresh,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final String projectStatus;
  final List<String> nextActions;
  final List<ApprovalPrompt> approvals;
  final ValueChanged<ApprovalPrompt> onOpenApproval;
  final VoidCallback onPromote;
  final VoidCallback onRefresh;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: tok.surface,
        border: Border(left: BorderSide(color: tok.border)),
      ),
      child: ListView(
        padding: const EdgeInsets.all(18),
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  copy.pick('Context', '情境'),
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 17,
                    fontWeight: FontWeight.w900,
                  ),
                ),
              ),
              IconButton(
                tooltip: copy.refresh,
                onPressed: onRefresh,
                icon: const Icon(Icons.refresh_rounded),
              ),
            ],
          ),
          if (approvals.isNotEmpty) ...[
            const SizedBox(height: 12),
            Text(
              copy.pick('Needs attention', '需要處理'),
              style: TextStyle(
                color: tok.warning,
                fontSize: 12,
                fontWeight: FontWeight.w800,
              ),
            ),
            const SizedBox(height: 8),
            for (final approval in approvals) ...[
              _ApprovalCard(
                tok: tok,
                copy: copy,
                approval: approval,
                accent: accent,
                onTap: () => onOpenApproval(approval),
              ),
              const SizedBox(height: 8),
            ],
          ],
          const SizedBox(height: 16),
          Text(
            copy.projectStatus,
            style: TextStyle(
              color: tok.muted,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
          const SizedBox(height: 8),
          Container(
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              color: tok.raised,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(18),
            ),
            child: Text(
              projectStatus.isEmpty
                  ? copy.pick('No project context yet.', '目前沒有專案情境。')
                  : projectStatus,
              style: TextStyle(color: tok.text, fontSize: 13, height: 1.45),
            ),
          ),
          if (nextActions.isNotEmpty) ...[
            const SizedBox(height: 14),
            for (final action in nextActions)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Padding(
                      padding: const EdgeInsets.only(top: 4),
                      child: Icon(
                        Icons.arrow_right_rounded,
                        color: accent,
                        size: 18,
                      ),
                    ),
                    const SizedBox(width: 4),
                    Expanded(
                      child: Text(
                        action,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 12.5,
                          height: 1.4,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
          ],
          const SizedBox(height: 12),
          OutlinedButton.icon(
            onPressed: onPromote,
            icon: const Icon(Icons.upload_file_rounded),
            label: Text(copy.promoteSession),
          ),
        ],
      ),
    );
  }
}
