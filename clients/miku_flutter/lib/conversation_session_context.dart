part of 'conversation_screen.dart';

class _SessionContextDrawer extends StatelessWidget {
  const _SessionContextDrawer({
    required this.session,
    required this.catalog,
    required this.loading,
    required this.error,
    required this.changingModeId,
    required this.onRetry,
    required this.onSelectMode,
    required this.onSetLocked,
    required this.onEndSession,
  });

  final MikuSession? session;
  final ModeCatalog? catalog;
  final bool loading;
  final String? error;
  final String? changingModeId;
  final VoidCallback onRetry;
  final ValueChanged<ModeProfile> onSelectMode;
  final ValueChanged<bool> onSetLocked;
  final VoidCallback onEndSession;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final current = session;
    final ended = current?.status == 'ended';
    return Drawer(
      key: const Key('session-context-drawer'),
      backgroundColor: Theme.of(context).colorScheme.surface,
      child: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 10, 20, 8),
              child: Row(
                children: [
                  IconButton(
                    key: const Key('close-session-context'),
                    tooltip: '關閉對話狀態',
                    onPressed: () => Navigator.of(context).pop(),
                    icon: const Icon(Icons.close_rounded),
                  ),
                  const SizedBox(width: 4),
                  Expanded(
                    child: Text(
                      '對話狀態',
                      style: Theme.of(context).textTheme.titleLarge?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ],
              ),
            ),
            Divider(height: 1, color: palette.outline),
            Expanded(
              child: ListView(
                key: const Key('session-context-list'),
                padding: const EdgeInsets.fromLTRB(16, 16, 16, 24),
                children: [
                  if (current == null)
                    const _DrawerLoadingState(label: '載入對話狀態…')
                  else ...[
                    _SessionSummaryCard(session: current),
                    const SizedBox(height: 18),
                    Text(
                      'Mode',
                      style: Theme.of(context).textTheme.titleMedium?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const SizedBox(height: 4),
                    Text(
                      'Mode 只改變這段對話的工作方式；Miku 的身份不會改變。',
                      style: Theme.of(
                        context,
                      ).textTheme.bodySmall?.copyWith(color: palette.muted),
                    ),
                    const SizedBox(height: 12),
                    if (loading && catalog == null)
                      const _DrawerLoadingState(label: '載入 Mode…')
                    else if (error != null && catalog == null)
                      _DrawerErrorState(error: error!, onRetry: onRetry)
                    else if (catalog != null)
                      for (final mode in catalog!.modes)
                        _ModeTile(
                          mode: mode,
                          selected: mode.id == current.mode,
                          changing: changingModeId == mode.id,
                          enabled: !ended && changingModeId == null,
                          onTap: () => onSelectMode(mode),
                        ),
                    if (error != null && catalog != null) ...[
                      const SizedBox(height: 8),
                      _DriveInlineError(message: error!, onRetry: onRetry),
                    ],
                    const SizedBox(height: 8),
                    SwitchListTile.adaptive(
                      key: const Key('mode-lock-toggle'),
                      contentPadding: EdgeInsets.zero,
                      value: current.locked,
                      onChanged:
                          ended || changingModeId != null ? null : onSetLocked,
                      title: const Text('鎖定目前 Mode'),
                      subtitle: Text(
                        current.locked
                            ? '保持目前 Mode，直到你解除鎖定。'
                            : '允許對話流程依明確規則調整。',
                      ),
                      secondary:
                          changingModeId == current.mode
                              ? const SizedBox.square(
                                dimension: 20,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                ),
                              )
                              : Icon(
                                current.locked
                                    ? Icons.lock_outline_rounded
                                    : Icons.lock_open_rounded,
                              ),
                    ),
                    const SizedBox(height: 10),
                    _ModeDetails(session: current, catalog: catalog),
                    const SizedBox(height: 24),
                    OutlinedButton.icon(
                      key: const Key('end-session'),
                      onPressed: ended ? null : onEndSession,
                      style: OutlinedButton.styleFrom(
                        foregroundColor: Theme.of(context).colorScheme.error,
                      ),
                      icon: const Icon(Icons.stop_circle_outlined),
                      label: Text(ended ? '對話已結束' : '結束這段對話'),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _SessionSummaryCard extends StatelessWidget {
  const _SessionSummaryCard({required this.session});

  final MikuSession session;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final scope = _projectIdFromScope(session.defaultScope);
    return Container(
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
              _PresenceMark(active: session.status != 'ended'),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  session.status == 'ended' ? '已結束的對話' : '目前對話',
                  style: Theme.of(
                    context,
                  ).textTheme.labelLarge?.copyWith(fontWeight: FontWeight.w600),
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          _ContextFact(
            icon: Icons.workspaces_outline,
            label: '範圍',
            value: scope == null ? 'Global' : 'Project · $scope',
          ),
          const SizedBox(height: 6),
          _ContextFact(
            icon: Icons.fingerprint_rounded,
            label: 'Session',
            value: _shortSessionId(session.id),
          ),
        ],
      ),
    );
  }
}

class _ContextFact extends StatelessWidget {
  const _ContextFact({
    required this.icon,
    required this.label,
    required this.value,
  });

  final IconData icon;
  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final muted = _Palette.of(context).muted;
    return Row(
      children: [
        Icon(icon, size: 17, color: muted),
        const SizedBox(width: 8),
        Text('$label：', style: TextStyle(color: muted)),
        Expanded(
          child: Text(value, maxLines: 1, overflow: TextOverflow.ellipsis),
        ),
      ],
    );
  }
}

class _ModeTile extends StatelessWidget {
  const _ModeTile({
    required this.mode,
    required this.selected,
    required this.changing,
    required this.enabled,
    required this.onTap,
  });

  final ModeProfile mode;
  final bool selected;
  final bool changing;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      selected: selected,
      button: !selected,
      label: '${mode.label} Mode${selected ? '，目前使用' : ''}',
      child: ListTile(
        key: Key('mode-${mode.id}'),
        contentPadding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
        leading:
            changing
                ? const SizedBox.square(
                  dimension: 20,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
                : Icon(
                  selected ? Icons.check_circle_rounded : Icons.circle_outlined,
                  color: selected ? palette.miku : palette.muted,
                ),
        title: Text(mode.label),
        subtitle:
            mode.description.isEmpty
                ? null
                : Text(
                  mode.description,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                ),
        enabled: selected || enabled,
        onTap: selected || !enabled ? null : onTap,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
        selected: selected,
        selectedTileColor: palette.miku.withValues(alpha: 0.08),
      ),
    );
  }
}

class _ModeDetails extends StatelessWidget {
  const _ModeDetails({required this.session, required this.catalog});

  final MikuSession session;
  final ModeCatalog? catalog;

  @override
  Widget build(BuildContext context) {
    final profile = catalog?.find(session.mode);
    final skills =
        session.activeSkills.isEmpty
            ? profile?.activeSkills ?? const <String>[]
            : session.activeSkills;
    final capabilities = profile?.capabilities ?? const <String>[];
    final palette = _Palette.of(context);
    return ExpansionTile(
      key: const Key('mode-details'),
      tilePadding: EdgeInsets.zero,
      childrenPadding: const EdgeInsets.only(bottom: 10),
      title: const Text('Mode 詳細資料'),
      subtitle: Text(
        profile?.capabilityClass ?? 'conversation',
        style: TextStyle(color: palette.muted),
      ),
      children: [
        _DetailGroup(
          label: '語氣上限',
          values: [profile?.voiceCap ?? session.voiceCap],
        ),
        _DetailGroup(label: 'Active skills', values: skills),
        _DetailGroup(label: 'Capabilities', values: capabilities),
      ],
    );
  }
}

class _DetailGroup extends StatelessWidget {
  const _DetailGroup({required this.label, required this.values});

  final String label;
  final List<String> values;

  @override
  Widget build(BuildContext context) {
    final visible = values.where((value) => value.trim().isNotEmpty).toList();
    return Padding(
      padding: const EdgeInsets.only(top: 10),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 92,
            child: Text(
              label,
              style: Theme.of(context).textTheme.labelMedium?.copyWith(
                color: _Palette.of(context).muted,
              ),
            ),
          ),
          Expanded(child: Text(visible.isEmpty ? '無' : visible.join('\n'))),
        ],
      ),
    );
  }
}

MikuSession _sessionWithMode(
  MikuSession session, {
  required ModeProfile profile,
  required bool locked,
}) {
  return MikuSession(
    id: session.id,
    status: session.status,
    mode: profile.id,
    label: profile.label,
    voiceCap: profile.voiceCap,
    defaultScope: session.defaultScope,
    activeSkills: profile.activeSkills,
    lastEventId: session.lastEventId,
    locked: locked,
  );
}

MikuSession _copySessionWithLock(MikuSession session, bool locked) {
  return MikuSession(
    id: session.id,
    status: session.status,
    mode: session.mode,
    label: session.label,
    voiceCap: session.voiceCap,
    defaultScope: session.defaultScope,
    activeSkills: session.activeSkills,
    lastEventId: session.lastEventId,
    locked: locked,
  );
}

MikuSession _sessionFromModeEvent(
  MikuSession session,
  Map<String, Object?> data,
) {
  final mode = _string(data['mode']);
  final label = _string(data['label']);
  final voiceCap = _string(data['voiceCap'] ?? data['voice_cap']);
  final rawSkills = data['activeSkills'] ?? data['active_skills'];
  final skills =
      rawSkills is List
          ? rawSkills.map((skill) => skill.toString()).toList()
          : session.activeSkills;
  final lockSource =
      data.containsKey('lockSource') ? data['lockSource'] : data['lock_source'];
  return MikuSession(
    id: session.id,
    status: session.status,
    mode: mode.isEmpty ? session.mode : mode,
    label: label.isEmpty ? session.label : label,
    voiceCap: voiceCap.isEmpty ? session.voiceCap : voiceCap,
    defaultScope: session.defaultScope,
    activeSkills: skills,
    lastEventId: session.lastEventId,
    locked: lockSource != null && _string(lockSource).isNotEmpty,
  );
}

String _shortSessionId(String id) {
  if (id.length <= 12) return id;
  return '${id.substring(0, 8)}…${id.substring(id.length - 4)}';
}

MikuSession _copySessionWithStatus(MikuSession session, String status) {
  return MikuSession(
    id: session.id,
    status: status,
    mode: session.mode,
    label: session.label,
    voiceCap: session.voiceCap,
    defaultScope: session.defaultScope,
    activeSkills: session.activeSkills,
    lastEventId: session.lastEventId,
    locked: session.locked,
  );
}
