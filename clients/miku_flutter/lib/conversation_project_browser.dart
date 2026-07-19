part of 'conversation_screen.dart';

class _ProjectBrowserLocation {
  const _ProjectBrowserLocation({required this.uri, required this.label});

  final String uri;
  final String label;
}

class _ProjectBrowserModel {
  const _ProjectBrowserModel({
    required this.projects,
    required this.activeProjectId,
    required this.switchingProjectId,
    required this.previewingResourceUri,
    required this.path,
    required this.entries,
    required this.catalogLoading,
    required this.browserLoading,
    required this.error,
  });

  final List<ProjectCatalogEntry>? projects;
  final String? activeProjectId;
  final String? switchingProjectId;
  final String? previewingResourceUri;
  final List<_ProjectBrowserLocation> path;
  final List<MikuResourceEntry>? entries;
  final bool catalogLoading;
  final bool browserLoading;
  final String? error;
}

class _ProjectBrowserView extends StatelessWidget {
  const _ProjectBrowserView({
    required this.model,
    required this.overview,
    required this.sessionEnded,
    required this.onRetryCatalog,
    required this.onRetryBrowser,
    required this.onSelectProject,
    required this.onOpenEntry,
    required this.onUp,
  });

  final _ProjectBrowserModel model;
  final ProjectOverview? overview;
  final bool sessionEnded;
  final VoidCallback onRetryCatalog;
  final VoidCallback onRetryBrowser;
  final ValueChanged<ProjectCatalogEntry> onSelectProject;
  final ValueChanged<MikuResourceEntry> onOpenEntry;
  final VoidCallback onUp;

  @override
  Widget build(BuildContext context) {
    final projects = model.projects;
    if (model.catalogLoading && projects == null) {
      return const _DrawerLoadingState(label: '載入 Project…');
    }
    if (model.error != null && projects == null) {
      return _DrawerErrorState(error: model.error!, onRetry: onRetryCatalog);
    }
    if (projects == null) return const SizedBox.shrink();
    if (projects.isEmpty) {
      return const _DrawerEmptyState(text: '尚未連結任何 Project。');
    }
    if (model.path.isEmpty) {
      return _ProjectCatalogList(
        projects: projects,
        activeProjectId: model.activeProjectId,
        switchingProjectId: model.switchingProjectId,
        sessionEnded: sessionEnded,
        error: model.error,
        onRetry: onRetryCatalog,
        onSelect: onSelectProject,
      );
    }
    return _ProjectDirectoryView(
      model: model,
      overview: overview,
      onRetry: onRetryBrowser,
      onOpenEntry: onOpenEntry,
      onUp: onUp,
    );
  }
}

class _ProjectCatalogList extends StatelessWidget {
  const _ProjectCatalogList({
    required this.projects,
    required this.activeProjectId,
    required this.switchingProjectId,
    required this.sessionEnded,
    required this.error,
    required this.onRetry,
    required this.onSelect,
  });

  final List<ProjectCatalogEntry> projects;
  final String? activeProjectId;
  final String? switchingProjectId;
  final bool sessionEnded;
  final String? error;
  final VoidCallback onRetry;
  final ValueChanged<ProjectCatalogEntry> onSelect;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Column(
      key: const Key('drawer-project-content'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (error != null) _DrawerErrorState(error: error!, onRetry: onRetry),
        for (final project in projects)
          Padding(
            padding: const EdgeInsets.only(bottom: 4),
            child: Semantics(
              button: true,
              selected: project.id == activeProjectId,
              label:
                  '${project.id} Project${project.id == activeProjectId ? '，目前使用中' : ''}',
              child: ListTile(
                key: Key('project-${project.id}'),
                minTileHeight: 52,
                dense: true,
                selected: project.id == activeProjectId,
                selectedTileColor: palette.miku.withValues(alpha: 0.10),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(10),
                ),
                leading: const Icon(Icons.folder_copy_outlined, size: 21),
                title: Text(
                  project.id,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                subtitle: Text(
                  project.id == activeProjectId
                      ? '目前對話'
                      : sessionEnded
                      ? '請先開新對話'
                      : project.memoryScope,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                trailing:
                    switchingProjectId == project.id
                        ? const SizedBox.square(
                          dimension: 18,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                        : project.id == activeProjectId
                        ? Icon(
                          Icons.check_rounded,
                          size: 18,
                          color: palette.miku,
                        )
                        : const Icon(Icons.chevron_right_rounded, size: 20),
                enabled:
                    switchingProjectId == null &&
                    (!sessionEnded || project.id == activeProjectId),
                onTap: () => onSelect(project),
              ),
            ),
          ),
      ],
    );
  }
}

class _ProjectDirectoryView extends StatelessWidget {
  const _ProjectDirectoryView({
    required this.model,
    required this.overview,
    required this.onRetry,
    required this.onOpenEntry,
    required this.onUp,
  });

  final _ProjectBrowserModel model;
  final ProjectOverview? overview;
  final VoidCallback onRetry;
  final ValueChanged<MikuResourceEntry> onOpenEntry;
  final VoidCallback onUp;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final location = model.path.last;
    final entries = model.entries;
    return Container(
      key: const Key('drawer-project-content'),
      decoration: BoxDecoration(
        color: palette.userBubble,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: palette.outline),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              IconButton(
                key: const Key('project-browser-up'),
                tooltip: model.path.length == 1 ? '返回 Project 清單' : '上一層',
                onPressed: model.browserLoading ? null : onUp,
                icon: const Icon(Icons.arrow_back_rounded, size: 20),
              ),
              Expanded(
                child: Text(
                  location.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.labelLarge,
                ),
              ),
              if (model.browserLoading)
                const Padding(
                  padding: EdgeInsets.only(right: 12),
                  child: SizedBox.square(
                    dimension: 16,
                    child: CircularProgressIndicator(strokeWidth: 1.8),
                  ),
                ),
            ],
          ),
          Divider(height: 1, color: palette.outline),
          if (model.path.length == 1 && overview != null)
            _ProjectOverviewSummary(overview: overview!),
          if (model.error != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(10, 6, 6, 2),
              child: _DrawerErrorState(error: model.error!, onRetry: onRetry),
            ),
          if (model.browserLoading && entries == null)
            const _DrawerLoadingState(label: '讀取資料夾…')
          else if (entries == null)
            const SizedBox.shrink()
          else if (entries.isEmpty)
            const _DrawerEmptyState(text: '這個資料夾是空的。')
          else
            for (final entry in entries)
              _ProjectResourceTile(
                entry: entry,
                loading: model.previewingResourceUri == entry.uri,
                enabled:
                    !model.browserLoading &&
                    model.previewingResourceUri == null,
                onTap: () => onOpenEntry(entry),
              ),
        ],
      ),
    );
  }
}

class _ProjectOverviewSummary extends StatelessWidget {
  const _ProjectOverviewSummary({required this.overview});

  final ProjectOverview overview;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(overview.status, style: Theme.of(context).textTheme.bodySmall),
          if (overview.nextActions.isNotEmpty) ...[
            const SizedBox(height: 7),
            Text(
              '下一步',
              style: Theme.of(context).textTheme.labelSmall?.copyWith(
                color: palette.muted,
                fontWeight: FontWeight.w600,
              ),
            ),
            for (final action in overview.nextActions.take(3))
              Padding(
                padding: const EdgeInsets.only(top: 3),
                child: Text(
                  '• $action',
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ),
          ],
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
                  tooltip: '關閉檔案預覽',
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
              child: DecoratedBox(
                decoration: BoxDecoration(
                  border: Border.all(color: palette.outline),
                  borderRadius: BorderRadius.circular(12),
                ),
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
            ),
          ],
        ),
      ),
    );
  }
}

String? _projectIdFromScope(String scope) {
  if (!scope.startsWith('project:')) return null;
  final id = scope.substring('project:'.length).trim();
  return id.isEmpty ? null : id;
}

MikuSession _sessionWithScope(MikuSession session, String scope) {
  return MikuSession(
    id: session.id,
    status: session.status,
    mode: session.mode,
    label: session.label,
    voiceCap: session.voiceCap,
    defaultScope: scope,
    activeSkills: session.activeSkills,
    lastEventId: session.lastEventId,
    locked: session.locked,
  );
}

String _friendlyProjectError(Object error) {
  final message = error.toString();
  if (message.contains('409') || message.contains('has ended')) {
    return '這段對話已結束；請先開新對話再切換 Project。';
  }
  if (message.contains('404') || message.contains('active project')) {
    return 'Project 已解除連結或目前不可用，請重新載入清單。';
  }
  return 'Project 暫時讀不到，請再試一次。';
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

String _formatBytes(int bytes) {
  if (bytes < 1024) return '$bytes B';
  if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KB';
  return '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MB';
}
