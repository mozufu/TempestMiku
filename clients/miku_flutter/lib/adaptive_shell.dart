part of 'main.dart';

// Adaptive shell for Tempest Miku. The shell exposes two ChatGPT-style
// drawers (sessions on the left, context + settings on the right) over the
// always-present chat surface. There is no bottom navigation bar or rail:
// every width keeps the chat thread centered and routes sessions/drive/
// context/settings through the drawers.

class _MikuLeftDrawer extends StatelessWidget {
  const _MikuLeftDrawer({
    required this.tok,
    required this.copy,
    required this.currentSessionId,
    required this.loadSessions,
    required this.onSelect,
    required this.onNewSession,
    required this.onDrive,
    required this.refreshToken,
  });

  final _Tok tok;
  final _UiCopy copy;
  final String? currentSessionId;
  final Future<List<SessionSummary>> Function() loadSessions;
  final void Function(String sessionId) onSelect;
  final VoidCallback onNewSession;
  final VoidCallback onDrive;
  final Object? refreshToken;

  @override
  Widget build(BuildContext context) {
    return Drawer(
      backgroundColor: tok.surface,
      width: _drawerWidth(context),
      child: SafeArea(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(8, 8, 8, 8),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(8, 6, 8, 4),
                child: Row(
                  children: [
                    const MikuBrandBadge(size: 36),
                    const SizedBox(width: 11),
                    Expanded(
                      child: Text(
                        copy.sessions,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 17,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    IconButton(
                      tooltip: copy.newSession,
                      onPressed: onNewSession,
                      icon: const Icon(Icons.add),
                    ),
                  ],
                ),
              ),
              Divider(color: tok.border, height: 1),
              Expanded(
                child: _SessionHistorySheet(
                  tok: tok,
                  copy: copy,
                  currentSessionId: currentSessionId,
                  loadSessions: loadSessions,
                  onSelect: onSelect,
                  onNewSession: onNewSession,
                  embedded: true,
                  refreshToken: refreshToken,
                ),
              ),
              Divider(color: tok.border, height: 1),
              ListTile(
                leading: Icon(Icons.folder_outlined, color: tok.muted),
                title: Text(
                  copy.driveFeed,
                  style: TextStyle(color: tok.text, fontWeight: FontWeight.w700),
                ),
                onTap: onDrive,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _MikuRightDrawer extends StatelessWidget {
  const _MikuRightDrawer({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.projectStatus,
    required this.nextActions,
    required this.approvals,
    required this.onOpenApproval,
    required this.onPromote,
    required this.onRefresh,
    required this.themeMode,
    required this.onThemeModeChanged,
    required this.onLanguageToggle,
    required this.onModeSettings,
    required this.onServerTarget,
    required this.onDisconnect,
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
  final ThemeMode themeMode;
  final ValueChanged<ThemeMode> onThemeModeChanged;
  final VoidCallback onLanguageToggle;
  final VoidCallback onModeSettings;
  final VoidCallback? onServerTarget;
  final VoidCallback? onDisconnect;

  @override
  Widget build(BuildContext context) {
    return Drawer(
      backgroundColor: tok.surface,
      width: _drawerWidth(context),
      child: SafeArea(
        child: ListView(
          padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 4),
          children: [
            _ContextContent(
              tok: tok,
              copy: copy,
              accent: accent,
              projectStatus: projectStatus,
              nextActions: nextActions,
              approvals: approvals,
              onOpenApproval: onOpenApproval,
              onPromote: onPromote,
              onRefresh: onRefresh,
            ),
            const SizedBox(height: 4),
            Divider(color: tok.border, height: 1),
            const SizedBox(height: 4),
            _OverflowContent(
              tok: tok,
              copy: copy,
              themeMode: themeMode,
              onThemeModeChanged: onThemeModeChanged,
              onLanguageToggle: onLanguageToggle,
              onModeSettings: onModeSettings,
              onServerTarget: onServerTarget,
              onDisconnect: onDisconnect,
            ),
          ],
        ),
      ),
    );
  }
}

double _drawerWidth(BuildContext context) {
  final width = MediaQuery.of(context).size.width;
  return math.min(width * 0.86, 360);
}

class _ContextContent extends StatelessWidget {
  const _ContextContent({
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
    return Padding(
      padding: const EdgeInsets.fromLTRB(14, 14, 14, 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
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
            const SizedBox(height: 4),
            _NeedsAttentionPill(
              tok: tok,
              copy: copy,
              count: approvals.length,
              onTap: () => onOpenApproval(approvals.first),
            ),
          ],
          const SizedBox(height: 12),
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
            const SizedBox(height: 12),
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
          const SizedBox(height: 10),
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

class _NeedsAttentionPill extends StatelessWidget {
  const _NeedsAttentionPill({
    required this.tok,
    required this.copy,
    required this.count,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final int count;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final text = copy.pick(
      count == 1 ? 'Miku needs your approval' : '$count approvals need attention',
      count == 1 ? 'Miku 需要你的核可' : '$count 個操作等待核可',
    );
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Material(
        color: tok.warning.withValues(alpha: 0.13),
        shape: RoundedRectangleBorder(
          side: BorderSide(color: tok.warning.withValues(alpha: 0.5)),
          borderRadius: BorderRadius.circular(18),
        ),
        child: InkWell(
          borderRadius: BorderRadius.circular(18),
          onTap: onTap,
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 52),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
              child: Row(
                children: [
                  Icon(Icons.shield_outlined, color: tok.warning, size: 21),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      text,
                      style: TextStyle(
                        color: tok.text,
                        fontSize: 13.5,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                  ),
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