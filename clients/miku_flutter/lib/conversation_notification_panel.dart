part of 'conversation_notifications.dart';

/// Low-frequency settings UI. It intentionally exposes no endpoint, token, or
/// provider secret and labels sync only as evidence observed during this app
/// launch.
class BackgroundNotificationsSettingsPanel extends StatefulWidget {
  const BackgroundNotificationsSettingsPanel({
    required this.coordinator,
    super.key,
  });

  final BackgroundNotificationCoordinator coordinator;

  @override
  State<BackgroundNotificationsSettingsPanel> createState() =>
      _BackgroundNotificationsSettingsPanelState();
}

class _BackgroundNotificationsSettingsPanelState
    extends State<BackgroundNotificationsSettingsPanel> {
  bool _detailExpanded = false;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: widget.coordinator,
      builder: (context, _) {
        final snapshot = widget.coordinator.snapshot;
        final visual = _NotificationStatusVisual.from(snapshot, context);
        final muted = TmTokens.of(context).muted;
        return DecoratedBox(
          decoration: BoxDecoration(
            border: Border.all(color: Theme.of(context).dividerColor),
            borderRadius: BorderRadius.circular(14),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              ConstrainedBox(
                constraints: const BoxConstraints(minHeight: 64),
                child: SwitchListTile.adaptive(
                  key: const Key('background-notifications-switch'),
                  value: snapshot.localOptIn,
                  onChanged:
                      snapshot.busy ||
                              snapshot.permission ==
                                  BackgroundNotificationPermission.unsupported
                          ? null
                          : widget.coordinator.setEnabled,
                  title: const Text('背景通知'),
                  subtitle: const Text('只在 App 不在前景時提醒；核准內容仍需回到 App 確認。'),
                  secondary: const Icon(Icons.notifications_none_rounded),
                ),
              ),
              const Divider(height: 1),
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 12, 12, 12),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(visual.icon, color: visual.color, size: 20),
                        const SizedBox(width: 10),
                        Expanded(
                          child: Text(
                            visual.title,
                            key: const Key('background-notifications-status'),
                            style: Theme.of(context).textTheme.labelLarge,
                          ),
                        ),
                        if (!snapshot.busy &&
                            ((snapshot.localOptIn &&
                                    !snapshot.syncedThisLaunch) ||
                                snapshot.syncState ==
                                    BackgroundNotificationSyncState
                                        .serverCleanupUnconfirmed))
                          SizedBox(
                            height: 44,
                            child: TextButton(
                              key: const Key('retry-background-notifications'),
                              onPressed: widget.coordinator.retrySync,
                              child: const Text('重試'),
                            ),
                          ),
                      ],
                    ),
                    Padding(
                      padding: const EdgeInsets.only(left: 30),
                      child: InkWell(
                        key: const Key(
                          'notification-technical-detail-toggle',
                        ),
                        onTap:
                            () => setState(
                              () => _detailExpanded = !_detailExpanded,
                            ),
                        borderRadius: BorderRadius.circular(8),
                        child: Padding(
                          padding: const EdgeInsets.symmetric(vertical: 6),
                          child: Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Text(
                                '技術細節',
                                style: Theme.of(
                                  context,
                                ).textTheme.bodySmall?.copyWith(color: muted),
                              ),
                              Icon(
                                _detailExpanded
                                    ? Icons.expand_less_rounded
                                    : Icons.expand_more_rounded,
                                size: 16,
                                color: muted,
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                    if (_detailExpanded)
                      Padding(
                        padding: const EdgeInsets.only(left: 30, top: 2),
                        child: Text(
                          visual.detail,
                          key: const Key(
                            'background-notifications-status-detail',
                          ),
                          style: Theme.of(
                            context,
                          ).textTheme.bodySmall?.copyWith(color: muted),
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _NotificationStatusVisual {
  const _NotificationStatusVisual({
    required this.icon,
    required this.color,
    required this.title,
    required this.detail,
  });

  final IconData icon;
  final Color color;
  final String title;
  final String detail;

  factory _NotificationStatusVisual.from(
    BackgroundNotificationSnapshot snapshot,
    BuildContext context,
  ) {
    final scheme = Theme.of(context).colorScheme;
    final muted = scheme.onSurfaceVariant;
    return switch (snapshot.syncState) {
      BackgroundNotificationSyncState.loading => _NotificationStatusVisual(
        icon: Icons.hourglass_empty_rounded,
        color: muted,
        title: '正在讀取這台裝置的狀態',
        detail: '不會在啟動時要求通知權限。',
      ),
      BackgroundNotificationSyncState.off => _NotificationStatusVisual(
        icon: Icons.notifications_off_outlined,
        color: muted,
        title: '這台裝置已在本機關閉',
        detail: '目前不會建立新的背景通知註冊。',
      ),
      BackgroundNotificationSyncState.permissionBlocked =>
        _NotificationStatusVisual(
          icon: Icons.block_rounded,
          color: scheme.error,
          title: '系統通知權限未開啟',
          detail: 'TempestMiku 沒有在背景顯示通知的系統權限。',
        ),
      BackgroundNotificationSyncState.permissionUnknown =>
        _NotificationStatusVisual(
          icon: Icons.help_outline_rounded,
          color: muted,
          title: '通知權限狀態未知',
          detail: '這台裝置目前無法讀取系統通知權限。',
        ),
      BackgroundNotificationSyncState.waitingEndpoint =>
        _NotificationStatusVisual(
          icon: Icons.hourglass_top_rounded,
          color: scheme.primary,
          title: '正在等待通知服務提供位址',
          detail: '尚未收到可同步到伺服器的 UnifiedPush 位址。',
        ),
      BackgroundNotificationSyncState.syncing => _NotificationStatusVisual(
        icon: Icons.sync_rounded,
        color: scheme.primary,
        title: '正在同步這台裝置',
        detail: '只有成功收到伺服器回條後才會顯示已同步。',
      ),
      BackgroundNotificationSyncState.syncedThisLaunch =>
        _NotificationStatusVisual(
          icon: Icons.check_circle_outline_rounded,
          color: scheme.primary,
          title: '本次啟動已同步',
          detail: '這表示本次 PUT 已收到回條，不代表日後伺服器狀態。',
        ),
      BackgroundNotificationSyncState.serverUnavailable =>
        _NotificationStatusVisual(
          icon: Icons.cloud_off_outlined,
          color: scheme.error,
          title: '伺服器目前無法同步',
          detail: '這台裝置可能尚未配對，或伺服器暫時無法使用。',
        ),
      BackgroundNotificationSyncState.distributorUnavailable =>
        _NotificationStatusVisual(
          icon: Icons.portable_wifi_off_rounded,
          color: scheme.error,
          title: '裝置通知服務目前無法使用',
          detail: '本機通知提供者沒有回應；未顯示或傳送任何位址內容。',
        ),
      BackgroundNotificationSyncState.serverCleanupUnconfirmed =>
        _NotificationStatusVisual(
          icon: Icons.warning_amber_rounded,
          color: scheme.error,
          title: '已在本機關閉；伺服器清理未確認',
          detail: '這台裝置已停止接收，稍後可在有網路時重試伺服器清理。',
        ),
      BackgroundNotificationSyncState.transitioningAuthority =>
        _NotificationStatusVisual(
          icon: Icons.sync_lock_rounded,
          color: scheme.primary,
          title: '正在切換裝置權限',
          detail: '通知佇列與回覆權限已暫停，等待配對結果。',
        ),
    };
  }
}
