part of 'conversation_screen.dart';

/// §30: A project is a first-class, server-owned entity with a subject — not a linked-folder alias.
/// A folder is an optional 0..n attachment; a project may be planning-only with no folder. This page
/// is entity-first: it lists project entities, creates/archives them, assigns the current session to
/// one (scope switch), and browses any attached linked folders. Linking a folder is a Miku-mediated
/// approval-gated host call (§30.3), surfaced here as guidance rather than a direct client action.
class _ProjectPage extends StatefulWidget {
  const _ProjectPage({
    required this.client,
    required this.session,
    required this.sessionEnded,
    required this.onScopeChanged,
  });

  final MikuSessionClient client;
  final MikuSession session;
  final bool sessionEnded;

  /// Reports a committed memory-scope change back to the conversation so the composer and Drive
  /// reflect the session's new project (or Global).
  final ValueChanged<String> onScopeChanged;

  @override
  State<_ProjectPage> createState() => _ProjectPageState();
}

class _ProjectPageState extends State<_ProjectPage> {
  List<ProjectCatalogEntry>? _projects;
  ProjectOverview? _overview;
  List<MikuResourceEntry>? _entries;
  final List<_ProjectBrowserLocation> _path = [];
  String _scope = 'global';
  bool _catalogLoading = false;
  bool _browserLoading = false;
  String? _switchingProjectId;
  bool _switchingToGlobal = false;
  String? _previewingUri;
  String? _error;
  bool _busy = false;

  String? get _activeProjectId => _projectIdFromScope(_scope);

  @override
  void initState() {
    super.initState();
    _scope = widget.session.defaultScope;
    unawaited(_loadCatalog());
  }

  Future<void> _loadCatalog() async {
    if (_catalogLoading) return;
    setState(() {
      _catalogLoading = true;
      _error = null;
    });
    try {
      final catalog = await widget.client.listProjects();
      if (!mounted) return;
      setState(() => _projects = catalog);
      final active =
          catalog.where((item) => item.id == _activeProjectId).firstOrNull;
      if (active != null) await _loadRoot(active);
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _catalogLoading = false);
    }
  }

  Future<void> _selectProject(ProjectCatalogEntry project) async {
    if (_switchingProjectId != null || _switchingToGlobal) return;
    if (widget.sessionEnded && project.id != _activeProjectId) return;
    if (project.id == _activeProjectId) {
      await _loadRoot(project);
      return;
    }
    setState(() {
      _switchingProjectId = project.id;
      _error = null;
    });
    try {
      final scope = await widget.client.setSessionScope(
        widget.session.id,
        project.memoryScope,
      );
      if (!mounted) return;
      if (scope != project.memoryScope) {
        throw StateError('server selected unexpected project scope $scope');
      }
      widget.onScopeChanged(scope);
      setState(() {
        _scope = scope;
        _overview = null;
        _entries = null;
        _path.clear();
      });
      await _loadRoot(project);
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _switchingProjectId = null);
    }
  }

  Future<void> _selectGlobalScope() async {
    if (_switchingProjectId != null || _switchingToGlobal) return;
    if (widget.sessionEnded && _activeProjectId != null) return;
    if (_scope == 'global') return;
    setState(() {
      _switchingToGlobal = true;
      _error = null;
    });
    try {
      final scope = await widget.client.setSessionScope(
        widget.session.id,
        'global',
      );
      if (!mounted) return;
      if (scope != 'global') {
        throw StateError('server selected unexpected global scope $scope');
      }
      widget.onScopeChanged(scope);
      setState(() {
        _scope = scope;
        _overview = null;
        _entries = null;
        _path.clear();
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _switchingToGlobal = false);
    }
  }

  Future<void> _createProject() async {
    if (_busy || widget.sessionEnded) return;
    final created = await showDialog<ProjectCatalogEntry>(
      context: context,
      builder: (context) => _CreateProjectDialog(client: widget.client),
    );
    if (created == null || !mounted) return;
    setState(() {
      final next = [...?_projects];
      if (!next.any((project) => project.id == created.id)) next.add(created);
      _projects = next;
    });
    await _selectProject(created);
  }

  Future<void> _archiveProject(ProjectCatalogEntry project) async {
    if (_busy) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder:
          (context) => AlertDialog(
            title: Text('封存 ${project.title}？'),
            content: const Text(
              '封存後這個 Project 不會出現在選單，記憶會保留但停止寫入。這是唯一會停用記憶範圍的動作。',
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(false),
                child: const Text('取消'),
              ),
              FilledButton(
                onPressed: () => Navigator.of(context).pop(true),
                child: const Text('封存'),
              ),
            ],
          ),
    );
    if (confirmed != true || !mounted) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.client.archiveProject(project.id);
      if (!mounted) return;
      if (_activeProjectId == project.id) {
        await _selectGlobalScope();
      }
      await _loadCatalog();
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _loadRoot(ProjectCatalogEntry project) async {
    if (_browserLoading) return;
    setState(() {
      _browserLoading = true;
      _error = null;
    });
    try {
      // §30: a folderless project has no linked root; browse its project views instead.
      final rootUri =
          project.hasLinkedFolder ? project.rootUri : project.projectUri;
      final overview = await widget.client.projectOverview(widget.session.id);
      final entries = await widget.client.listResources(
        widget.session.id,
        rootUri,
      );
      if (!mounted || _activeProjectId != project.id) return;
      setState(() {
        _overview = overview;
        _entries = entries;
        _path
          ..clear()
          ..add(_ProjectBrowserLocation(uri: rootUri, label: project.title));
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _browserLoading = false);
    }
  }

  Future<void> _openEntry(MikuResourceEntry entry) async {
    if (entry.isDirectory) {
      await _loadLocation(
        _ProjectBrowserLocation(uri: entry.uri, label: entry.name),
        push: true,
      );
    } else if (entry.isFile) {
      await _openFile(entry);
    }
  }

  Future<void> _loadLocation(
    _ProjectBrowserLocation location, {
    required bool push,
  }) async {
    if (_browserLoading) return;
    setState(() {
      _browserLoading = true;
      _error = null;
    });
    try {
      final entries = await widget.client.listResources(
        widget.session.id,
        location.uri,
      );
      if (!mounted) return;
      setState(() {
        _entries = entries;
        if (push) _path.add(location);
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _browserLoading = false);
    }
  }

  Future<void> _goUp() async {
    if (_path.length <= 1) {
      setState(() {
        _overview = null;
        _entries = null;
        _path.clear();
        _error = null;
      });
      return;
    }
    final parent = _path[_path.length - 2];
    setState(() => _path.removeLast());
    await _loadLocation(parent, push: false);
  }

  Future<void> _openFile(MikuResourceEntry entry) async {
    if (!entry.isFile || _previewingUri != null) return;
    setState(() {
      _previewingUri = entry.uri;
      _error = null;
    });
    try {
      final resource = await widget.client.resolveResource(
        widget.session.id,
        entry.uri,
      );
      if (!mounted) return;
      setState(() => _previewingUri = null);
      await showModalBottomSheet<void>(
        context: context,
        useSafeArea: true,
        isScrollControlled: true,
        showDragHandle: true,
        builder:
            (context) => _ProjectFileSheet(entry: entry, resource: resource),
      );
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _previewingUri = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Project'),
        actions: [
          IconButton(
            key: const Key('project-create'),
            tooltip: '新 Project',
            onPressed: (_busy || widget.sessionEnded) ? null : _createProject,
            icon: const Icon(Icons.add_rounded),
          ),
        ],
      ),
      body: SafeArea(child: _buildBody()),
    );
  }

  Widget _buildBody() {
    final projects = _projects;
    if (_catalogLoading && projects == null) {
      return const _DrawerLoadingState(label: '載入 Project…');
    }
    if (_error != null && projects == null) {
      return _DrawerErrorState(error: _error!, onRetry: _loadCatalog);
    }
    if (projects == null) return const SizedBox.shrink();
    if (_path.isEmpty) {
      return _ProjectCatalogList(
        projects: projects,
        activeProjectId: _activeProjectId,
        switchingProjectId: _switchingProjectId,
        switchingToGlobal: _switchingToGlobal,
        sessionEnded: widget.sessionEnded,
        busy: _busy,
        error: _error,
        onRetry: _loadCatalog,
        onSelectGlobalScope: _selectGlobalScope,
        onSelect: _selectProject,
        onArchive: _archiveProject,
      );
    }
    return _ProjectDirectoryView(
      path: _path,
      entries: _entries,
      overview: _overview,
      browserLoading: _browserLoading,
      previewingResourceUri: _previewingUri,
      error: _error,
      onRetry: () => _loadLocation(_path.last, push: false),
      onOpenEntry: _openEntry,
      onUp: _goUp,
    );
  }
}

class _ProjectBrowserLocation {
  const _ProjectBrowserLocation({required this.uri, required this.label});

  final String uri;
  final String label;
}

class _CreateProjectDialog extends StatefulWidget {
  const _CreateProjectDialog({required this.client});

  final MikuSessionClient client;

  @override
  State<_CreateProjectDialog> createState() => _CreateProjectDialogState();
}

class _CreateProjectDialogState extends State<_CreateProjectDialog> {
  final TextEditingController _controller = TextEditingController();
  bool _submitting = false;
  String? _error;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    final title = _controller.text.trim();
    if (title.isEmpty || _submitting) return;
    setState(() {
      _submitting = true;
      _error = null;
    });
    try {
      final slug = _projectIdForTitle(title);
      final created = await widget.client.createProject(slug, title: title);
      if (!mounted) return;
      Navigator.of(context).pop(created);
    } catch (_) {
      if (!mounted) return;
      setState(() {
        _submitting = false;
        _error = '無法建立 Project，請換個名稱再試。';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('新 Project'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text('Project 是一個有主題的實體，不一定要連結資料夾。'),
          const SizedBox(height: 12),
          TextField(
            key: const Key('create-project-title'),
            controller: _controller,
            autofocus: true,
            decoration: const InputDecoration(
              labelText: '名稱',
              hintText: '例如：旅遊規劃',
            ),
            onSubmitted: (_) => _submit(),
          ),
          if (_error != null) ...[
            const SizedBox(height: 8),
            Text(
              _error!,
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                color: Theme.of(context).colorScheme.error,
              ),
            ),
          ],
        ],
      ),
      actions: [
        TextButton(
          onPressed: _submitting ? null : () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(
          key: const Key('create-project-submit'),
          onPressed: _submitting ? null : _submit,
          child:
              _submitting
                  ? const SizedBox.square(
                    dimension: 16,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                  : const Text('建立'),
        ),
      ],
    );
  }
}

String _projectIdForTitle(String title) {
  final ascii = title
      .toLowerCase()
      .replaceAll(RegExp(r'[^a-z0-9]+'), '-')
      .replaceAll(RegExp(r'(^-+|-+$)'), '');
  if (ascii.isNotEmpty) return ascii;
  final codePoints = title.runes
      .where((codePoint) => String.fromCharCode(codePoint).trim().isNotEmpty)
      .map((codePoint) => codePoint.toRadixString(16))
      .join('-');
  return 'project-$codePoints';
}

class _ProjectCatalogList extends StatelessWidget {
  const _ProjectCatalogList({
    required this.projects,
    required this.activeProjectId,
    required this.switchingProjectId,
    required this.switchingToGlobal,
    required this.sessionEnded,
    required this.busy,
    required this.error,
    required this.onRetry,
    required this.onSelectGlobalScope,
    required this.onSelect,
    required this.onArchive,
  });

  final List<ProjectCatalogEntry> projects;
  final String? activeProjectId;
  final String? switchingProjectId;
  final bool switchingToGlobal;
  final bool sessionEnded;
  final bool busy;
  final String? error;
  final VoidCallback onRetry;
  final VoidCallback onSelectGlobalScope;
  final ValueChanged<ProjectCatalogEntry> onSelect;
  final ValueChanged<ProjectCatalogEntry> onArchive;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final busySwitch = switchingProjectId != null || switchingToGlobal || busy;
    return ListView(
      key: const Key('project-page-content'),
      padding: const EdgeInsets.fromLTRB(12, 12, 12, 20),
      children: [
        if (error != null)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: _DrawerErrorState(error: error!, onRetry: onRetry),
          ),
        Padding(
          padding: const EdgeInsets.only(bottom: 4),
          child: Semantics(
            button: true,
            selected: activeProjectId == null,
            label:
                'Global 範圍${activeProjectId == null ? '，目前使用中' : '，不綁定 Project'}',
            child: ListTile(
              key: const Key('project-global-scope'),
              minTileHeight: 52,
              selected: activeProjectId == null,
              selectedTileColor: palette.miku.withValues(alpha: 0.10),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(10),
              ),
              leading: const Icon(Icons.public_rounded, size: 21),
              title: const Text('Global'),
              subtitle: Text(
                activeProjectId == null ? '目前對話' : '不綁定 Project 記憶',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              trailing:
                  switchingToGlobal
                      ? const SizedBox.square(
                        dimension: 18,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                      : activeProjectId == null
                      ? Icon(Icons.check_rounded, size: 18, color: palette.miku)
                      : const Icon(Icons.chevron_right_rounded, size: 20),
              enabled:
                  !busySwitch && (!sessionEnded || activeProjectId == null),
              onTap: onSelectGlobalScope,
            ),
          ),
        ),
        if (projects.isEmpty)
          const _DrawerEmptyState(text: '尚未建立任何 Project。用右上角的「＋」新增一個。'),
        for (final project in projects)
          Padding(
            padding: const EdgeInsets.only(bottom: 4),
            child: Semantics(
              button: true,
              selected: project.id == activeProjectId,
              label:
                  '${project.title} Project${project.id == activeProjectId ? '，目前使用中' : ''}',
              child: ListTile(
                key: Key('project-${project.id}'),
                minTileHeight: 52,
                selected: project.id == activeProjectId,
                selectedTileColor: palette.miku.withValues(alpha: 0.10),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(10),
                ),
                leading: Icon(
                  project.hasLinkedFolder
                      ? Icons.folder_copy_outlined
                      : Icons.workspaces_outline,
                  size: 21,
                ),
                title: Text(
                  project.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                subtitle: Text(
                  project.id == activeProjectId
                      ? '目前對話'
                      : sessionEnded
                      ? '請先開新對話'
                      : project.hasLinkedFolder
                      ? '已連結資料夾'
                      : '規劃用（無連結資料夾）',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                trailing:
                    switchingProjectId == project.id
                        ? const SizedBox.square(
                          dimension: 18,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                        : _ProjectTrailing(
                          projectId: project.id,
                          active: project.id == activeProjectId,
                          onArchive:
                              busySwitch ? null : () => onArchive(project),
                        ),
                enabled:
                    !busySwitch &&
                    (!sessionEnded || project.id == activeProjectId),
                onTap: () => onSelect(project),
              ),
            ),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 0),
          child: Text(
            '要把資料夾連結進 Project，直接在對話中告訴 Miku 路徑；連結需要你核准。',
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(color: palette.muted),
          ),
        ),
      ],
    );
  }
}

class _ProjectTrailing extends StatelessWidget {
  const _ProjectTrailing({
    required this.projectId,
    required this.active,
    required this.onArchive,
  });

  final String projectId;
  final bool active;
  final VoidCallback? onArchive;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (active)
          Icon(Icons.check_rounded, size: 18, color: palette.miku)
        else
          const Icon(Icons.chevron_right_rounded, size: 20),
        IconButton(
          key: Key('project-archive-$projectId'),
          tooltip: '封存 Project',
          visualDensity: VisualDensity.compact,
          onPressed: onArchive,
          icon: const Icon(Icons.archive_outlined, size: 18),
        ),
      ],
    );
  }
}

class _ProjectDirectoryView extends StatelessWidget {
  const _ProjectDirectoryView({
    required this.path,
    required this.entries,
    required this.overview,
    required this.browserLoading,
    required this.previewingResourceUri,
    required this.error,
    required this.onRetry,
    required this.onOpenEntry,
    required this.onUp,
  });

  final List<_ProjectBrowserLocation> path;
  final List<MikuResourceEntry>? entries;
  final ProjectOverview? overview;
  final bool browserLoading;
  final String? previewingResourceUri;
  final String? error;
  final VoidCallback onRetry;
  final ValueChanged<MikuResourceEntry> onOpenEntry;
  final VoidCallback onUp;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final location = path.last;
    final values = entries;
    return Column(
      key: const Key('project-page-content'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            IconButton(
              key: const Key('project-browser-up'),
              tooltip: path.length == 1 ? '返回 Project 清單' : '上一層',
              onPressed: browserLoading ? null : onUp,
              icon: const Icon(Icons.arrow_back_rounded, size: 20),
            ),
            Expanded(
              child: Text(
                location.label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.titleMedium,
              ),
            ),
            if (browserLoading)
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
        Expanded(
          child: ListView(
            padding: const EdgeInsets.fromLTRB(8, 6, 8, 20),
            children: [
              if (path.length == 1 && overview != null)
                _ProjectOverviewSummary(overview: overview!),
              if (error != null)
                Padding(
                  padding: const EdgeInsets.fromLTRB(6, 6, 6, 2),
                  child: _DrawerErrorState(error: error!, onRetry: onRetry),
                ),
              if (browserLoading && values == null)
                const _DrawerLoadingState(label: '讀取資料夾…')
              else if (values == null)
                const SizedBox.shrink()
              else if (values.isEmpty)
                const _DrawerEmptyState(text: '這個資料夾是空的。')
              else
                for (final entry in values)
                  _ProjectResourceTile(
                    entry: entry,
                    loading: previewingResourceUri == entry.uri,
                    enabled: !browserLoading && previewingResourceUri == null,
                    onTap: () => onOpenEntry(entry),
                  ),
            ],
          ),
        ),
      ],
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
          if (overview.openLoops.isNotEmpty) ...[
            const SizedBox(height: 8),
            _ProjectItemGroup(
              label: 'Open loops',
              icon: Icons.pending_actions_outlined,
              items: overview.openLoops,
            ),
          ],
          if (overview.decisions.isNotEmpty) ...[
            const SizedBox(height: 8),
            _ProjectItemGroup(
              label: 'Decisions',
              icon: Icons.rule_rounded,
              items: overview.decisions,
            ),
          ],
        ],
      ),
    );
  }
}

class _ProjectItemGroup extends StatelessWidget {
  const _ProjectItemGroup({
    required this.label,
    required this.icon,
    required this.items,
  });

  final String label;
  final IconData icon;
  final List<ProjectItem> items;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Icon(icon, size: 16, color: palette.muted),
            const SizedBox(width: 6),
            Text(
              label,
              style: Theme.of(context).textTheme.labelSmall?.copyWith(
                color: palette.muted,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
        for (final item in items.take(3))
          Padding(
            padding: const EdgeInsets.only(top: 3, left: 22),
            child: Text(
              item.text,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ),
      ],
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

String _friendlyProjectError(Object error) {
  final message = error.toString();
  if (message.contains('409') || message.contains('has ended')) {
    return '這段對話已結束；請先開新對話再切換 Project。';
  }
  if (message.contains('404') || message.contains('active project')) {
    return 'Project 目前不可用，請重新載入清單。';
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
