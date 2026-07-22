part of 'conversation_screen.dart';

/// §30 scope-detail surface: the per-scope detail opened from the project catalog for a
/// project or the pinned Global scope, presenting its 記憶 / 檔案 / 脈絡 tabs — memory reads,
/// linked-folder file browsing, and the project context overview.

class _ScopeDetailView extends StatefulWidget {
  const _ScopeDetailView({
    required this.title,
    required this.subtitle,
    required this.icon,
    required this.isGlobal,
    required this.atRoot,
    required this.initialTabIndex,
    required this.onTabChanged,
    required this.memory,
    required this.files,
    required this.context_,
    required this.onNewConversation,
    required this.onContinueConversation,
    required this.startingConversation,
  });

  final String title;
  final String subtitle;
  final IconData icon;
  final bool isGlobal;
  final bool atRoot;
  final int initialTabIndex;
  final ValueChanged<int> onTabChanged;
  final Widget memory;
  final Widget? files;
  final Widget? context_;
  final VoidCallback? onNewConversation;
  final VoidCallback onContinueConversation;
  final bool startingConversation;

  @override
  State<_ScopeDetailView> createState() => _ScopeDetailViewState();
}

class _ScopeDetailViewState extends State<_ScopeDetailView>
    with SingleTickerProviderStateMixin {
  late final TabController _tabs;
  late final List<_ScopeTab> _tabDefs;

  @override
  void initState() {
    super.initState();
    _tabDefs = [
      const _ScopeTab(label: '記憶', icon: Icons.psychology_outlined),
      if (widget.files != null)
        const _ScopeTab(label: '檔案', icon: Icons.folder_outlined),
      if (widget.context_ != null)
        const _ScopeTab(label: '脈絡', icon: Icons.layers_outlined),
    ];
    final start = widget.initialTabIndex.clamp(0, _tabDefs.length - 1);
    _tabs = TabController(
      length: _tabDefs.length,
      vsync: this,
      initialIndex: start,
    );
    _tabs.addListener(() {
      if (!_tabs.indexIsChanging) widget.onTabChanged(_tabs.index);
    });
  }

  @override
  void dispose() {
    _tabs.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    // A drilled-in subfolder shows only the file listing full-bleed (no tabs/header).
    if (!widget.atRoot) {
      return widget.files ?? const SizedBox.shrink();
    }
    final palette = _Palette.of(context);
    final views = <Widget>[
      widget.memory,
      if (widget.files != null) widget.files!,
      if (widget.context_ != null) widget.context_!,
    ];
    return Column(
      key: const Key('project-page-content'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _ScopeDetailHeader(
          title: widget.title,
          subtitle: widget.subtitle,
          icon: widget.icon,
          onNewConversation: widget.onNewConversation,
          onContinueConversation: widget.onContinueConversation,
          startingConversation: widget.startingConversation,
        ),
        TabBar(
          key: const Key('scope-detail-tabs'),
          controller: _tabs,
          labelColor: palette.miku,
          indicatorColor: palette.miku,
          tabs: [
            for (final tab in _tabDefs)
              Tab(
                height: 46,
                icon: Icon(tab.icon, size: 19),
                iconMargin: const EdgeInsets.only(bottom: 2),
                child: Text(tab.label),
              ),
          ],
        ),
        Divider(height: 1, color: palette.outline),
        Expanded(child: TabBarView(controller: _tabs, children: views)),
      ],
    );
  }
}

class _ScopeTab {
  const _ScopeTab({required this.label, required this.icon});
  final String label;
  final IconData icon;
}

class _ScopeDetailHeader extends StatelessWidget {
  const _ScopeDetailHeader({
    required this.title,
    required this.subtitle,
    required this.icon,
    required this.onNewConversation,
    required this.onContinueConversation,
    required this.startingConversation,
  });

  final String title;
  final String subtitle;
  final IconData icon;
  final VoidCallback? onNewConversation;
  final VoidCallback onContinueConversation;
  final bool startingConversation;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 20, 14),
      decoration: BoxDecoration(
        color: palette.miku.withValues(alpha: 0.07),
        border: Border(bottom: BorderSide(color: palette.outline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Container(
                width: 42,
                height: 42,
                decoration: BoxDecoration(
                  color: palette.miku.withValues(alpha: 0.15),
                  borderRadius: BorderRadius.circular(13),
                ),
                child: Icon(icon, color: palette.miku, size: 21),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.titleLarge?.copyWith(
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                    if (subtitle.trim().isNotEmpty)
                      Text(
                        subtitle,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(
                          context,
                        ).textTheme.bodySmall?.copyWith(color: palette.muted),
                      ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Row(
            children: [
              if (onNewConversation != null)
                Expanded(
                  child: FilledButton.icon(
                    key: const Key('project-new-conversation'),
                    onPressed: startingConversation ? null : onNewConversation,
                    icon:
                        startingConversation
                            ? const SizedBox.square(
                              dimension: 17,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                            : const Icon(Icons.add_comment_outlined, size: 18),
                    label: Text(startingConversation ? '建立中…' : '在這裡新增對話'),
                  ),
                ),
              if (onNewConversation != null) const SizedBox(width: 8),
              Expanded(
                child: TextButton.icon(
                  key: const Key('project-continue-conversation'),
                  onPressed:
                      startingConversation ? null : onContinueConversation,
                  icon: const Icon(Icons.arrow_back_rounded, size: 18),
                  label: const Text('回到目前對話'),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _ScopeMemoryTab extends StatelessWidget {
  const _ScopeMemoryTab({
    required this.scope,
    required this.policy,
    required this.canTogglePolicy,
    required this.policyBusy,
    required this.onPolicyChanged,
    required this.summaries,
    required this.chunks,
    required this.loading,
    required this.error,
    required this.previewingUri,
    required this.onRetry,
    required this.onOpen,
  });

  final String scope;
  final MikuMemoryPolicy policy;
  final bool canTogglePolicy;
  final bool policyBusy;
  final ValueChanged<MikuMemoryPolicy> onPolicyChanged;
  final List<MikuResourceEntry>? summaries;
  final List<MikuResourceEntry>? chunks;
  final bool loading;
  final String? error;
  final String? previewingUri;
  final VoidCallback onRetry;
  final ValueChanged<MikuResourceEntry> onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final summaryItems = summaries ?? const [];
    final chunkItems = chunks ?? const [];
    final loaded = summaries != null || chunks != null;
    final empty = loaded && summaryItems.isEmpty && chunkItems.isEmpty;
    return ListView(
      key: const Key('scope-memory-tab'),
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 28),
      children: [
        Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 760),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (canTogglePolicy) ...[
                  Text(
                    '這段對話的記憶範圍',
                    style: Theme.of(
                      context,
                    ).textTheme.labelLarge?.copyWith(color: palette.muted),
                  ),
                  const SizedBox(height: 8),
                  SegmentedButton<MikuMemoryPolicy>(
                    key: const Key('project-memory-policy'),
                    segments: const [
                      ButtonSegment(
                        value: MikuMemoryPolicy.project,
                        icon: Icon(Icons.workspaces_outline, size: 17),
                        label: Text('此 Project'),
                      ),
                      ButtonSegment(
                        value: MikuMemoryPolicy.global,
                        icon: Icon(Icons.public_rounded, size: 17),
                        label: Text('沿用全域'),
                      ),
                    ],
                    selected: {policy},
                    onSelectionChanged:
                        policyBusy
                            ? null
                            : (selection) => onPolicyChanged(selection.first),
                  ),
                  const SizedBox(height: 6),
                  Text(
                    policy == MikuMemoryPolicy.project
                        ? '寫入與回想都留在這個 Project；不會混入其它對話。'
                        : '這段對話沿用全域記憶，不寫入 Project 專屬範圍。',
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
                  const SizedBox(height: 18),
                ],
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Miku 記得什麼',
                        style: Theme.of(context).textTheme.titleMedium
                            ?.copyWith(fontWeight: FontWeight.w700),
                      ),
                    ),
                    if (loading)
                      const SizedBox.square(
                        dimension: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      ),
                  ],
                ),
                const SizedBox(height: 4),
                Text(
                  '整併後的摘要與可回想片段（§22），依範圍授權顯示。',
                  style: Theme.of(
                    context,
                  ).textTheme.bodySmall?.copyWith(color: palette.muted),
                ),
                if (error != null) ...[
                  const SizedBox(height: 12),
                  _DrawerErrorState(error: error!, onRetry: onRetry),
                ],
                const SizedBox(height: 12),
                if (!loaded && loading)
                  const _DrawerLoadingState(label: '讀取記憶…')
                else if (empty && error == null)
                  _ScopeMemoryEmptyState(
                    global:
                        !canTogglePolicy && policy == MikuMemoryPolicy.global,
                  )
                else ...[
                  if (summaryItems.isNotEmpty) ...[
                    _ScopeMemoryGroup(
                      label: '整併摘要',
                      icon: Icons.auto_awesome_outlined,
                      keyPrefix: 'memory-summary',
                      entries: summaryItems,
                      previewingUri: previewingUri,
                      onOpen: onOpen,
                    ),
                    const SizedBox(height: 16),
                  ],
                  if (chunkItems.isNotEmpty)
                    _ScopeMemoryGroup(
                      label: '可回想片段',
                      icon: Icons.article_outlined,
                      keyPrefix: 'memory-chunk',
                      entries: chunkItems,
                      previewingUri: previewingUri,
                      onOpen: onOpen,
                    ),
                ],
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class _ScopeMemoryGroup extends StatelessWidget {
  const _ScopeMemoryGroup({
    required this.label,
    required this.icon,
    required this.keyPrefix,
    required this.entries,
    required this.previewingUri,
    required this.onOpen,
  });

  final String label;
  final IconData icon;
  final String keyPrefix;
  final List<MikuResourceEntry> entries;
  final String? previewingUri;
  final ValueChanged<MikuResourceEntry> onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Icon(icon, size: 17, color: palette.muted),
            const SizedBox(width: 7),
            Text(
              '$label · ${entries.length}',
              style: Theme.of(
                context,
              ).textTheme.labelLarge?.copyWith(color: palette.muted),
            ),
          ],
        ),
        const SizedBox(height: 8),
        DecoratedBox(
          decoration: BoxDecoration(
            color: Theme.of(context).colorScheme.surface,
            border: Border.all(color: palette.outline),
            borderRadius: BorderRadius.circular(16),
          ),
          child: Column(
            children: [
              for (var index = 0; index < entries.length; index++) ...[
                _ScopeMemoryTile(
                  entry: entries[index],
                  keyPrefix: keyPrefix,
                  loading: previewingUri == entries[index].uri,
                  enabled: previewingUri == null,
                  onTap: () => onOpen(entries[index]),
                ),
                if (index != entries.length - 1)
                  Divider(height: 1, indent: 16, color: palette.outline),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

class _ScopeMemoryTile extends StatelessWidget {
  const _ScopeMemoryTile({
    required this.entry,
    required this.keyPrefix,
    required this.loading,
    required this.enabled,
    required this.onTap,
  });

  final MikuResourceEntry entry;
  final String keyPrefix;
  final bool loading;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final text =
        entry.title?.trim().isNotEmpty == true
            ? entry.title!.trim()
            : entry.name;
    final when = _friendlyMemoryTimestamp(entry.modifiedAt);
    return Semantics(
      button: true,
      enabled: enabled,
      child: ListTile(
        key: Key('$keyPrefix-${entry.uri}'),
        minTileHeight: 56,
        title: Text(text, maxLines: 2, overflow: TextOverflow.ellipsis),
        subtitle:
            when == null
                ? null
                : Text(
                  when,
                  style: TextStyle(color: palette.muted, fontSize: 12),
                ),
        trailing:
            loading
                ? const SizedBox.square(
                  dimension: 17,
                  child: CircularProgressIndicator(strokeWidth: 1.8),
                )
                : Icon(
                  Icons.chevron_right_rounded,
                  size: 19,
                  color: palette.muted,
                ),
        enabled: enabled,
        onTap: onTap,
      ),
    );
  }
}

class _ScopeMemoryEmptyState extends StatelessWidget {
  const _ScopeMemoryEmptyState({required this.global});

  final bool global;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      key: const Key('scope-memory-empty'),
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        border: Border.all(color: palette.outline),
        borderRadius: BorderRadius.circular(18),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.nights_stay_outlined, color: palette.muted),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              global
                  ? '全域記憶還是空的。多聊幾次，Miku 會在空檔整併出摘要。'
                  : '這個範圍還沒有整併出記憶。對話結束後 Miku 會在空檔整理。',
              style: Theme.of(
                context,
              ).textTheme.bodyMedium?.copyWith(color: palette.muted),
            ),
          ),
        ],
      ),
    );
  }
}

class _ScopeFilesTab extends StatelessWidget {
  const _ScopeFilesTab({
    required this.hasLinkedFolder,
    required this.folderCount,
    required this.entries,
    required this.loading,
    required this.previewingResourceUri,
    required this.error,
    required this.onRetry,
    required this.onOpenEntry,
    this.hideHeader = false,
  });

  final bool hasLinkedFolder;
  final int folderCount;
  final List<MikuResourceEntry>? entries;
  final bool loading;
  final String? previewingResourceUri;
  final String? error;
  final VoidCallback onRetry;
  final ValueChanged<MikuResourceEntry> onOpenEntry;
  final bool hideHeader;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final values = entries;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (loading) const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: LayoutBuilder(
            builder: (context, constraints) {
              final horizontal = constraints.maxWidth < 600 ? 16.0 : 28.0;
              return ListView(
                padding: EdgeInsets.fromLTRB(horizontal, 16, horizontal, 28),
                children: [
                  Center(
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 760),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          if (!hideHeader) ...[
                            Row(
                              children: [
                                Expanded(
                                  child: Text(
                                    '連結資料',
                                    style: Theme.of(context)
                                        .textTheme
                                        .titleMedium
                                        ?.copyWith(fontWeight: FontWeight.w700),
                                  ),
                                ),
                                Text(
                                  hasLinkedFolder
                                      ? '$folderCount 個資料夾'
                                      : '尚未連結',
                                  style: Theme.of(context).textTheme.labelMedium
                                      ?.copyWith(color: palette.muted),
                                ),
                              ],
                            ),
                            const SizedBox(height: 4),
                            Text(
                              hasLinkedFolder
                                  ? '只顯示你明確授權給這個 Project 的檔案。'
                                  : '這個 Project 可以先用來規劃；需要檔案時再請 Miku 連結。',
                              style: Theme.of(context).textTheme.bodySmall
                                  ?.copyWith(color: palette.muted),
                            ),
                            const SizedBox(height: 10),
                          ],
                          if (error != null) ...[
                            _DrawerErrorState(error: error!, onRetry: onRetry),
                            const SizedBox(height: 12),
                          ],
                          if (loading && values == null)
                            const _DrawerLoadingState(label: '讀取資料…')
                          else if (values == null)
                            const SizedBox.shrink()
                          else if (values.isEmpty)
                            _ProjectFilesEmptyState(
                              folderless: !hideHeader && !hasLinkedFolder,
                            )
                          else
                            DecoratedBox(
                              decoration: BoxDecoration(
                                color: Theme.of(context).colorScheme.surface,
                                border: Border.all(color: palette.outline),
                                borderRadius: BorderRadius.circular(18),
                              ),
                              child: Column(
                                children: [
                                  for (
                                    var index = 0;
                                    index < values.length;
                                    index++
                                  ) ...[
                                    _ProjectResourceTile(
                                      entry: values[index],
                                      loading:
                                          previewingResourceUri ==
                                          values[index].uri,
                                      enabled:
                                          !loading &&
                                          previewingResourceUri == null,
                                      onTap: () => onOpenEntry(values[index]),
                                    ),
                                    if (index != values.length - 1)
                                      Divider(
                                        height: 1,
                                        indent: 56,
                                        color: palette.outline,
                                      ),
                                  ],
                                ],
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
                ],
              );
            },
          ),
        ),
      ],
    );
  }
}

class _ProjectFilesEmptyState extends StatelessWidget {
  const _ProjectFilesEmptyState({required this.folderless});

  final bool folderless;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        border: Border.all(color: palette.outline),
        borderRadius: BorderRadius.circular(18),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            folderless ? Icons.link_off_rounded : Icons.folder_open_rounded,
            color: palette.muted,
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              folderless ? '還沒有連結資料。直接在對話裡告訴 Miku 要使用哪個資料夾。' : '這裡目前沒有檔案。',
              style: Theme.of(
                context,
              ).textTheme.bodyMedium?.copyWith(color: palette.muted),
            ),
          ),
        ],
      ),
    );
  }
}

class _ScopeContextTab extends StatelessWidget {
  const _ScopeContextTab({required this.overview});

  final ProjectOverview? overview;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final data = overview;
    if (data == null) {
      return const _DrawerLoadingState(label: '讀取脈絡…');
    }
    final actions =
        data.nextActions
            .where((action) => action.trim().isNotEmpty)
            .take(3)
            .toList();
    final hasAny =
        actions.isNotEmpty ||
        data.openLoops.isNotEmpty ||
        data.decisions.isNotEmpty;
    if (!hasAny) {
      return ListView(
        key: const Key('scope-context-tab'),
        padding: const EdgeInsets.fromLTRB(20, 16, 20, 28),
        children: [
          Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 760),
              child: Container(
                key: const Key('scope-context-empty'),
                padding: const EdgeInsets.all(18),
                decoration: BoxDecoration(
                  color: Theme.of(context).colorScheme.surface,
                  border: Border.all(color: palette.outline),
                  borderRadius: BorderRadius.circular(18),
                ),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Icon(Icons.layers_outlined, color: palette.muted),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(
                        '還沒整理出待處理或決定。回到對話，和 Miku 決定一個最小的下一步。',
                        style: Theme.of(
                          context,
                        ).textTheme.bodyMedium?.copyWith(color: palette.muted),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ],
      );
    }
    return ListView(
      key: const Key('scope-context-tab'),
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 28),
      children: [
        Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 760),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (actions.isNotEmpty) ...[
                  _ScopeContextGroup(
                    label: '接下來',
                    icon: Icons.arrow_forward_rounded,
                    accent: true,
                    texts: actions,
                  ),
                  const SizedBox(height: 16),
                ],
                if (data.openLoops.isNotEmpty) ...[
                  _ScopeContextGroup(
                    label: '待處理',
                    icon: Icons.pending_actions_outlined,
                    accent: false,
                    texts: data.openLoops.map((item) => item.text).toList(),
                  ),
                  const SizedBox(height: 16),
                ],
                if (data.decisions.isNotEmpty)
                  _ScopeContextGroup(
                    label: '已決定',
                    icon: Icons.rule_rounded,
                    accent: false,
                    texts: data.decisions.map((item) => item.text).toList(),
                  ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class _ScopeContextGroup extends StatelessWidget {
  const _ScopeContextGroup({
    required this.label,
    required this.icon,
    required this.accent,
    required this.texts,
  });

  final String label;
  final IconData icon;
  final bool accent;
  final List<String> texts;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color:
            accent
                ? palette.miku.withValues(alpha: 0.08)
                : Theme.of(context).colorScheme.surface,
        border: Border.all(
          color:
              accent ? palette.miku.withValues(alpha: 0.28) : palette.outline,
        ),
        borderRadius: BorderRadius.circular(18),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Icon(
                icon,
                size: 17,
                color: accent ? palette.miku : palette.muted,
              ),
              const SizedBox(width: 7),
              Text(
                label,
                style: Theme.of(context).textTheme.labelLarge?.copyWith(
                  color: accent ? palette.miku : palette.muted,
                  fontWeight: FontWeight.w700,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          for (final text in texts.take(5))
            Padding(
              padding: const EdgeInsets.only(bottom: 6, left: 24),
              child: Text(
                text,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.bodyMedium,
              ),
            ),
        ],
      ),
    );
  }
}

class _ProjectResourceTile extends StatelessWidget {
  const _ProjectResourceTile({
    required this.entry,
    required this.loading,
    required this.enabled,
    required this.onTap,
  });

  final MikuResourceEntry entry;
  final bool loading;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final supported = entry.isDirectory || entry.isFile;
    final size = entry.sizeBytes;
    return Semantics(
      button: supported,
      enabled: enabled && supported,
      label: '${entry.name}，${_resourceKindLabel(entry)}',
      child: ListTile(
        key: Key('project-resource-${entry.uri}'),
        minTileHeight: 48,
        dense: true,
        leading: Icon(_resourceIcon(entry), size: 20),
        title: Text(entry.name, maxLines: 1, overflow: TextOverflow.ellipsis),
        subtitle:
            supported && size == null
                ? null
                : Text(
                  supported ? _formatBytes(size!) : '不支援此項目類型',
                  maxLines: 1,
                ),
        trailing:
            loading
                ? const SizedBox.square(
                  dimension: 17,
                  child: CircularProgressIndicator(strokeWidth: 1.8),
                )
                : entry.isDirectory
                ? const Icon(Icons.chevron_right_rounded, size: 19)
                : null,
        enabled: enabled && supported,
        onTap: supported ? onTap : null,
      ),
    );
  }
}

class _ProjectFileSheet extends StatelessWidget {
  const _ProjectFileSheet({required this.entry, required this.resource});

  final MikuResourceEntry entry;
  final ResourcePreview resource;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final body = resource.content.isEmpty ? resource.preview : resource.content;
    return FractionallySizedBox(
      heightFactor: 0.86,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    resource.title?.trim().isNotEmpty == true
                        ? resource.title!
                        : entry.name,
                    key: const Key('project-file-title'),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.titleMedium?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                IconButton(
                  tooltip: '關閉預覽',
                  onPressed: () => Navigator.of(context).pop(),
                  icon: const Icon(Icons.close_rounded),
                ),
              ],
            ),
            Text(
              resource.uri,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
            const SizedBox(height: 5),
            Text(
              '${resource.mime} · ${_formatBytes(resource.sizeBytes)}',
              style: Theme.of(
                context,
              ).textTheme.labelSmall?.copyWith(color: palette.muted),
            ),
            if (resource.hasMore) ...[
              const SizedBox(height: 10),
              Semantics(
                liveRegion: true,
                child: Container(
                  key: const Key('project-file-truncated'),
                  padding: const EdgeInsets.symmetric(
                    horizontal: 10,
                    vertical: 8,
                  ),
                  decoration: BoxDecoration(
                    color: palette.miku.withValues(alpha: 0.10),
                    borderRadius: BorderRadius.circular(9),
                  ),
                  child: const Text('內容超過安全讀取上限，以下為目前載入的部分。'),
                ),
              ),
            ],
            const SizedBox(height: 12),
            Expanded(
              child: Scrollbar(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.all(14),
                  child: SelectableText(
                    body.isEmpty ? '沒有可顯示的文字內容。' : body,
                    key: const Key('project-file-content'),
                    style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                      fontFamily: 'monospace',
                      height: 1.45,
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

String? _friendlyMemoryTimestamp(String? raw) {
  if (raw == null || raw.trim().isEmpty) return null;
  final parsed = DateTime.tryParse(raw);
  if (parsed == null) return null;
  final now = DateTime.now();
  final delta = now.difference(parsed.toLocal());
  if (delta.inMinutes < 1) return '剛剛';
  if (delta.inHours < 1) return '${delta.inMinutes} 分鐘前';
  if (delta.inDays < 1) return '${delta.inHours} 小時前';
  if (delta.inDays < 7) return '${delta.inDays} 天前';
  final local = parsed.toLocal();
  final month = local.month.toString().padLeft(2, '0');
  final day = local.day.toString().padLeft(2, '0');
  return '${local.year}-$month-$day';
}

IconData _resourceIcon(MikuResourceEntry entry) {
  return switch (entry.kind) {
    'linked_folder' => Icons.folder_copy_outlined,
    'dir' => Icons.folder_outlined,
    'file' || 'text' => Icons.description_outlined,
    'symlink' => Icons.link_rounded,
    _ => Icons.help_outline_rounded,
  };
}

String _resourceKindLabel(MikuResourceEntry entry) {
  return switch (entry.kind) {
    'linked_folder' => 'Linked folder',
    'dir' => '資料夾',
    'file' || 'text' => '檔案',
    'symlink' => '符號連結，不可開啟',
    _ => '不支援的項目',
  };
}
