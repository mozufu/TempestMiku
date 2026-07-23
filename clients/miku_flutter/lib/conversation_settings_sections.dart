part of 'conversation_screen.dart';

class _ThemeModeSettings extends StatelessWidget {
  const _ThemeModeSettings({
    required this.mode,
    required this.saving,
    required this.error,
    required this.onChanged,
  });

  final ThemeMode mode;
  final bool saving;
  final String? error;
  final ValueChanged<ThemeMode> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          '預設跟隨裝置；你的選擇只保存在這台裝置。',
          style: Theme.of(
            context,
          ).textTheme.bodySmall?.copyWith(color: palette.muted),
        ),
        const SizedBox(height: 10),
        Semantics(
          container: true,
          label: '顯示主題',
          value: _themeModeLabel(mode),
          child: SegmentedButton<ThemeMode>(
            key: const Key('theme-mode-chooser'),
            expandedInsets: EdgeInsets.zero,
            segments: const [
              ButtonSegment(
                value: ThemeMode.system,
                icon: Icon(Icons.brightness_auto_rounded, size: 18),
                label: Text('系統', key: Key('theme-mode-system')),
              ),
              ButtonSegment(
                value: ThemeMode.light,
                icon: Icon(Icons.light_mode_outlined, size: 18),
                label: Text('淺色', key: Key('theme-mode-light')),
              ),
              ButtonSegment(
                value: ThemeMode.dark,
                icon: Icon(Icons.dark_mode_outlined, size: 18),
                label: Text('深色', key: Key('theme-mode-dark')),
              ),
            ],
            selected: {mode},
            showSelectedIcon: true,
            onSelectionChanged:
                saving ? null : (selection) => onChanged(selection.single),
            style: ButtonStyle(
              minimumSize: const WidgetStatePropertyAll(Size(0, 48)),
              textStyle: WidgetStatePropertyAll(
                Theme.of(context).textTheme.labelLarge,
              ),
            ),
          ),
        ),
        if (error != null) ...[
          const SizedBox(height: 8),
          Text(
            error!,
            key: const Key('theme-mode-error'),
            style: Theme.of(context).textTheme.bodySmall?.copyWith(
              color: Theme.of(context).colorScheme.error,
            ),
          ),
        ],
      ],
    );
  }
}

String _themeModeLabel(ThemeMode mode) => switch (mode) {
  ThemeMode.system => '跟隨系統',
  ThemeMode.light => '淺色',
  ThemeMode.dark => '深色',
};

class _PairingTargetRow extends StatelessWidget {
  const _PairingTargetRow({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 76,
            child: Text(label, style: Theme.of(context).textTheme.bodySmall),
          ),
          Expanded(
            child: SelectableText(
              value,
              style: Theme.of(
                context,
              ).textTheme.bodyMedium?.copyWith(fontWeight: FontWeight.w600),
            ),
          ),
        ],
      ),
    );
  }
}

class _SettingsSection extends StatelessWidget {
  const _SettingsSection({
    required this.title,
    required this.action,
    required this.child,
  });

  final String title;
  final Widget action;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                title,
                style: Theme.of(
                  context,
                ).textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w600),
              ),
            ),
            action,
          ],
        ),
        const SizedBox(height: 6),
        child,
      ],
    );
  }
}

class _SettingsLoadState extends StatelessWidget {
  const _SettingsLoadState({required this.error, required this.onRetry});

  final String? error;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    if (error != null) {
      return _DrawerErrorState(error: error!, onRetry: onRetry);
    }
    return const _DrawerLoadingState(label: '載入中…');
  }
}

class _DiagnosticsCard extends StatelessWidget {
  const _DiagnosticsCard({required this.readiness, required this.diagnostics});

  final ServerReadiness? readiness;
  final ServerDiagnostics? diagnostics;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    final isReady = readiness?.ready;
    return Container(
      key: const Key('server-diagnostics'),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        border: Border.all(color: palette.outline),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                isReady == true
                    ? Icons.check_circle_outline_rounded
                    : isReady == false
                    ? Icons.warning_amber_rounded
                    : Icons.help_outline_rounded,
                color:
                    isReady == true
                        ? palette.miku
                        : isReady == false
                        ? Theme.of(context).colorScheme.error
                        : palette.muted,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  isReady == true
                      ? '伺服器已就緒'
                      : isReady == false
                      ? '伺服器尚未就緒'
                      : '就緒狀態未知',
                  style: Theme.of(
                    context,
                  ).textTheme.labelLarge?.copyWith(fontWeight: FontWeight.w600),
                ),
              ),
            ],
          ),
          if (readiness != null && readiness!.detail.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(
              _friendlyReadinessDetail(readiness!),
              key: const Key('server-readiness-detail'),
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
          ],
          Theme(
            data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
            child: ExpansionTile(
              key: const Key('server-diagnostics-advanced'),
              tilePadding: EdgeInsets.zero,
              childrenPadding: const EdgeInsets.only(bottom: 4),
              title: Text(
                '進階（開發者）',
                style: Theme.of(
                  context,
                ).textTheme.labelMedium?.copyWith(color: palette.muted),
              ),
              children: [
                if (diagnostics != null) ...[
                  Align(
                    alignment: Alignment.centerLeft,
                    child: Text(
                      '執行角色：${diagnostics!.role}',
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: palette.muted,
                      ),
                    ),
                  ),
                  const SizedBox(height: 6),
                  SelectableText(
                    diagnostics!.baseUrl,
                    style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      fontFamily: 'monospace',
                      color: palette.muted,
                    ),
                  ),
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    children: [
                      _MetricChip(
                        label: '處理佇列',
                        value: diagnostics!.turnQueueDepth,
                      ),
                      _MetricChip(
                        label: '整理佇列',
                        value: diagnostics!.dreamQueueDepth,
                      ),
                      _MetricChip(
                        label: '排程',
                        value: diagnostics!.schedulerQueueDepth,
                      ),
                      _MetricChip(
                        label: '待核准',
                        value: diagnostics!.pendingApprovals,
                      ),
                      if (diagnostics!.pushQueueDepth != null)
                        _MetricChip(
                          label: '推播佇列',
                          value: diagnostics!.pushQueueDepth!,
                        ),
                    ],
                  ),
                ] else
                  Text(
                    '佇列深度目前不可用。',
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

String _friendlyReadinessDetail(ServerReadiness readiness) {
  final runtime = readiness.runtime;
  if (runtime.shuttingDown || readiness.status == 'draining') {
    return '伺服器正在排空既有工作，暫不接收新工作。';
  }
  if (runtime.postgres && !runtime.migrationsApplied) {
    return '資料庫遷移尚未完成，暫不接收新工作。';
  }
  if (runtime.workersEnabled && !runtime.postgres) {
    return '工作執行程式需要 Postgres，目前設定不完整。';
  }
  final memory = readiness.memory;
  if (memory != null && !memory.durableWritesReady) {
    final reason = memory.schema.reason;
    return reason == null || reason.isEmpty
        ? '持久記憶目前無法寫入。'
        : '持久記憶目前無法寫入：$reason';
  }
  if (readiness.ready) {
    if (memory == null) return '執行環境已可接收工作。';
    if (memory.denseRetrievalReady) {
      return '執行環境、持久記憶與語意檢索皆已就緒。';
    }
    return '執行環境與持久記憶已就緒；語意檢索目前使用降級路徑。';
  }
  return '伺服器回報狀態：${readiness.status.isEmpty ? '未知' : readiness.status}';
}

class _MetricChip extends StatelessWidget {
  const _MetricChip({required this.label, required this.value});

  final String label;
  final int value;

  @override
  Widget build(BuildContext context) {
    return Chip(
      label: Text('$label · $value'),
      visualDensity: VisualDensity.compact,
    );
  }
}

class _DeviceTile extends StatelessWidget {
  const _DeviceTile({
    required this.device,
    required this.identityKnown,
    required this.isCurrent,
    required this.revoking,
    required this.onRevoke,
  });

  final AuthDevice device;
  final bool identityKnown;
  final bool isCurrent;
  final bool revoking;
  final VoidCallback onRevoke;

  @override
  Widget build(BuildContext context) {
    return ListTile(
      key: Key('auth-device-${device.id}'),
      contentPadding: const EdgeInsets.symmetric(horizontal: 4),
      leading: Icon(_deviceIcon(device.platform)),
      title: Text(device.name),
      subtitle: Text(
        device.isActive
            ? '${device.platform} · ${_friendlyTimestamp(device.lastSeenAt)}'
            : '${device.platform} · 已於 ${_friendlyTimestamp(device.revokedAt!)} 撤銷',
      ),
      trailing:
          revoking
              ? const SizedBox.square(
                dimension: 20,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
              : !device.isActive
              ? const Chip(
                label: Text('已撤銷'),
                visualDensity: VisualDensity.compact,
              )
              : isCurrent
              ? Semantics(
                label: '目前使用中的裝置',
                child: const Chip(
                  key: Key('current-auth-device'),
                  label: Text('這台裝置'),
                  visualDensity: VisualDensity.compact,
                ),
              )
              : !identityKnown
              ? Semantics(
                label: '目前無法確認這是不是這台裝置，暫不提供撤銷',
                child: const Chip(
                  label: Text('暫不可撤銷'),
                  visualDensity: VisualDensity.compact,
                ),
              )
              : IconButton(
                tooltip: '撤銷 ${device.name}',
                onPressed: onRevoke,
                icon: const Icon(Icons.link_off_rounded),
              ),
    );
  }
}

IconData _deviceIcon(String platform) {
  return switch (platform.toLowerCase()) {
    'android' || 'ios' => Icons.smartphone_rounded,
    'web' => Icons.language_rounded,
    _ => Icons.devices_other_rounded,
  };
}
