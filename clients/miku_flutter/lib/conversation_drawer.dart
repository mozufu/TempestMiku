part of 'conversation_screen.dart';

class _ConversationDrawer extends StatelessWidget {
  const _ConversationDrawer({
    required this.onOpenSettings,
    required this.onOpenResources,
    required this.onOpenReviewedChanges,
    required this.onNewConversation,
    required this.currentSessionId,
    required this.currentSessionEnded,
    required this.onOpenDrive,
    required this.onOpenProject,
    required this.onOpenHistory,
  });

  final VoidCallback onOpenSettings;
  final VoidCallback onOpenResources;
  final VoidCallback onOpenReviewedChanges;
  final VoidCallback onNewConversation;
  final String? currentSessionId;
  final bool currentSessionEnded;
  final VoidCallback onOpenDrive;
  final VoidCallback onOpenProject;
  final VoidCallback onOpenHistory;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final hasSession = currentSessionId != null;
    return Drawer(
      key: const Key('left-conversation-drawer'),
      backgroundColor: Theme.of(context).colorScheme.surface,
      child: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 10, 8, 8),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      'Miku',
                      key: const Key('left-drawer-title'),
                      style: Theme.of(context).textTheme.titleLarge?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                  IconButton(
                    key: const Key('close-left-drawer'),
                    tooltip: '關閉對話選單',
                    onPressed: () => Navigator.of(context).pop(),
                    icon: const Icon(Icons.close_rounded),
                  ),
                ],
              ),
            ),
            Divider(height: 1, color: palette.outline),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(8, 14, 8, 12),
                children: [
                  _DrawerPageDestination(
                    pageKey: const Key('drawer-drive'),
                    icon: Icons.folder_open_rounded,
                    label: 'Drive',
                    subtitle: 'Miku 的空間',
                    enabled: hasSession,
                    onTap: onOpenDrive,
                  ),
                  _DrawerPageDestination(
                    pageKey: const Key('drawer-project'),
                    icon: Icons.workspaces_outline,
                    label: 'Project',
                    subtitle: '主題實體與工作範圍',
                    enabled: hasSession,
                    onTap: onOpenProject,
                  ),
                  _DrawerPageDestination(
                    pageKey: const Key('drawer-history'),
                    icon: Icons.history_rounded,
                    label: 'History',
                    subtitle: '過往對話與指派',
                    enabled: true,
                    onTap: onOpenHistory,
                  ),
                  ListTile(
                    key: const Key('drawer-resources'),
                    minTileHeight: 52,
                    leading: const Icon(Icons.inventory_2_outlined),
                    title: const Text('Resources'),
                    subtitle: const Text('進階唯讀檢視'),
                    trailing: const Icon(Icons.chevron_right_rounded),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(12),
                    ),
                    onTap: () {
                      Navigator.of(context).pop();
                      onOpenResources();
                    },
                  ),
                  ListTile(
                    key: const Key('drawer-reviewed-changes'),
                    minTileHeight: 52,
                    leading: const Icon(Icons.rule_folder_outlined),
                    title: const Text('經審核的變更'),
                    subtitle: const Text('記憶、guidance 與 rollback'),
                    trailing: const Icon(Icons.chevron_right_rounded),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(12),
                    ),
                    enabled: hasSession && !currentSessionEnded,
                    onTap:
                        !hasSession || currentSessionEnded
                            ? null
                            : () {
                              Navigator.of(context).pop();
                              onOpenReviewedChanges();
                            },
                  ),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(14, 12, 14, 18),
              child: Row(
                children: [
                  Expanded(
                    child: OutlinedButton.icon(
                      key: const Key('drawer-settings'),
                      onPressed: () {
                        Navigator.of(context).pop();
                        onOpenSettings();
                      },
                      icon: const Icon(Icons.settings_outlined, size: 19),
                      label: const Text('設定'),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: FilledButton.icon(
                      key: const Key('drawer-new-conversation'),
                      onPressed: () {
                        onNewConversation();
                        Navigator.of(context).pop();
                      },
                      icon: const Icon(Icons.add_comment_outlined, size: 19),
                      label: const Text('新對話'),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _DrawerPageDestination extends StatelessWidget {
  const _DrawerPageDestination({
    required this.pageKey,
    required this.icon,
    required this.label,
    required this.subtitle,
    required this.enabled,
    required this.onTap,
  });

  final Key pageKey;
  final IconData icon;
  final String label;
  final String subtitle;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Semantics(
        button: true,
        label: label,
        child: ListTile(
          key: pageKey,
          minTileHeight: 52,
          leading: Icon(icon),
          title: Text(label),
          subtitle: Text(subtitle),
          trailing: const Icon(Icons.chevron_right_rounded, size: 20),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
          enabled: enabled,
          onTap:
              enabled
                  ? () {
                    Navigator.of(context).pop();
                    onTap();
                  }
                  : null,
        ),
      ),
    );
  }
}

class _DrawerLoadingState extends StatelessWidget {
  const _DrawerLoadingState({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      child: Row(
        children: [
          const SizedBox.square(
            dimension: 15,
            child: CircularProgressIndicator(strokeWidth: 1.8),
          ),
          const SizedBox(width: 10),
          Text(label, style: Theme.of(context).textTheme.bodySmall),
        ],
      ),
    );
  }
}

class _DrawerErrorState extends StatelessWidget {
  const _DrawerErrorState({required this.error, required this.onRetry});

  final String error;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.error;
    return Semantics(
      liveRegion: true,
      child: Row(
        children: [
          Expanded(
            child: Text(
              error,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: color),
            ),
          ),
          IconButton(
            tooltip: '重試',
            onPressed: onRetry,
            icon: const Icon(Icons.refresh_rounded, size: 19),
          ),
        ],
      ),
    );
  }
}

class _DrawerEmptyState extends StatelessWidget {
  const _DrawerEmptyState({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 7, 12, 11),
      child: Text(
        text,
        style: Theme.of(
          context,
        ).textTheme.bodySmall?.copyWith(color: _Palette.of(context).muted),
      ),
    );
  }
}
