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
    required this.onNewConversation,
  });

  final MikuSessionClient client;
  final MikuSession session;
  final bool sessionEnded;

  /// Reports a committed memory-scope change back to the conversation so the composer and Drive
  /// reflect the session's new project (or Global).
  final ValueChanged<String> onScopeChanged;

  final Future<bool> Function(ProjectCatalogEntry project) onNewConversation;

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
  MikuResourceEntry? _failedPreviewEntry;
  String? _error;
  bool _startingConversation = false;
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
        _failedPreviewEntry = null;
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
        _failedPreviewEntry = null;
      });
      return;
    }
    final parent = _path[_path.length - 2];
    setState(() {
      _path.removeLast();
      _failedPreviewEntry = null;
    });
    await _loadLocation(parent, push: false);
  }

  void _continueConversation() {
    Navigator.of(context).pop();
  }

  Future<void> _startConversation(ProjectCatalogEntry project) async {
    if (_startingConversation) return;
    setState(() {
      _startingConversation = true;
      _error = null;
    });
    final created = await widget.onNewConversation(project);
    if (!mounted) return;
    if (created) {
      Navigator.of(context).pop();
      return;
    }
    setState(() {
      _startingConversation = false;
      _error = '無法在這個 Project 建立對話，請再試一次。';
    });
  }

  Future<void> _openFile(MikuResourceEntry entry) async {
    if (!entry.isFile || _previewingUri != null) return;
    setState(() {
      _previewingUri = entry.uri;
      _error = null;
      _failedPreviewEntry = null;
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
      setState(() {
        _error = _friendlyProjectError(error);
        _failedPreviewEntry = entry;
      });
    } finally {
      if (mounted) setState(() => _previewingUri = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: _path.isEmpty,
      onPopInvokedWithResult: (didPop, result) {
        if (!didPop) unawaited(_goUp());
      },
      child: Scaffold(
        appBar: AppBar(
          leading:
              _path.isEmpty
                  ? const BackButton()
                  : BackButton(
                    key: const Key('project-browser-up'),
                    onPressed: _browserLoading ? null : _goUp,
                  ),
          title: Text(_path.isEmpty ? 'Projects' : _path.last.label),
          actions: [
            if (_path.isEmpty)
              IconButton(
                key: const Key('project-create'),
                tooltip: '新 Project',
                onPressed:
                    (_busy || widget.sessionEnded) ? null : _createProject,
                icon: const Icon(Icons.add_rounded),
              ),
          ],
        ),
        body: SafeArea(child: _buildBody()),
      ),
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
    final activeProject =
        projects.where((project) => project.id == _activeProjectId).firstOrNull;
    if (activeProject == null) {
      return _DrawerErrorState(error: '找不到這個 Project。', onRetry: _loadCatalog);
    }
    return _ProjectDirectoryView(
      project: activeProject,
      path: _path,
      entries: _entries,
      overview: _overview,
      browserLoading: _browserLoading,
      previewingResourceUri: _previewingUri,
      error: _error,
      onRetry: () {
        final failed = _failedPreviewEntry;
        if (failed != null) {
          unawaited(_openFile(failed));
          return;
        }
        unawaited(_loadLocation(_path.last, push: false));
      },
      onOpenEntry: _openEntry,
      startingConversation: _startingConversation,
      onNewConversation: () => _startConversation(activeProject),
      onContinueConversation: _continueConversation,
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
    return LayoutBuilder(
      builder: (context, constraints) {
        final horizontal = constraints.maxWidth < 600 ? 16.0 : 28.0;
        return ListView(
          key: const Key('project-page-content'),
          padding: EdgeInsets.fromLTRB(horizontal, 12, horizontal, 28),
          children: [
            Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 760),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Text(
                      '把對話放回它正在前進的事情裡。',
                      style: Theme.of(context).textTheme.titleLarge?.copyWith(
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                    const SizedBox(height: 6),
                    Text(
                      '選一個 Project，Miku 會沿用它的記憶、下一步與連結資料。',
                      style: Theme.of(
                        context,
                      ).textTheme.bodyMedium?.copyWith(color: palette.muted),
                    ),
                    if (error != null) ...[
                      const SizedBox(height: 12),
                      _DrawerErrorState(error: error!, onRetry: onRetry),
                    ],
                    const SizedBox(height: 20),
                    Text(
                      '你的 Projects',
                      style: Theme.of(
                        context,
                      ).textTheme.labelLarge?.copyWith(color: palette.muted),
                    ),
                    const SizedBox(height: 8),
                    if (projects.isEmpty)
                      const _DrawerEmptyState(
                        text: '還沒有 Project。按右上角的「＋」建立第一個。',
                      )
                    else
                      for (final project in projects) ...[
                        _ProjectCatalogCard(
                          project: project,
                          active: project.id == activeProjectId,
                          loading: switchingProjectId == project.id,
                          enabled:
                              !busySwitch &&
                              (!sessionEnded || project.id == activeProjectId),
                          sessionEnded: sessionEnded,
                          onTap: () => onSelect(project),
                          onArchive:
                              busySwitch ? null : () => onArchive(project),
                        ),
                        const SizedBox(height: 10),
                      ],
                    const SizedBox(height: 10),
                    Text(
                      '其他對話',
                      style: Theme.of(
                        context,
                      ).textTheme.labelLarge?.copyWith(color: palette.muted),
                    ),
                    const SizedBox(height: 8),
                    Semantics(
                      button: true,
                      selected: activeProjectId == null,
                      label:
                          'Global 範圍${activeProjectId == null ? '，目前使用中' : '，不綁定 Project'}',
                      child: ListTile(
                        key: const Key('project-global-scope'),
                        minTileHeight: 58,
                        selected: activeProjectId == null,
                        selectedTileColor: palette.miku.withValues(alpha: 0.09),
                        tileColor: Theme.of(context).colorScheme.surface,
                        shape: RoundedRectangleBorder(
                          side: BorderSide(
                            color:
                                activeProjectId == null
                                    ? palette.miku.withValues(alpha: 0.45)
                                    : palette.outline,
                          ),
                          borderRadius: BorderRadius.circular(16),
                        ),
                        leading: const Icon(Icons.public_rounded, size: 21),
                        title: const Text('不使用 Project'),
                        subtitle: Text(
                          activeProjectId == null ? '目前對話' : '使用全域記憶',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                        trailing:
                            switchingToGlobal
                                ? const SizedBox.square(
                                  dimension: 18,
                                  child: CircularProgressIndicator(
                                    strokeWidth: 2,
                                  ),
                                )
                                : activeProjectId == null
                                ? Icon(
                                  Icons.check_circle_rounded,
                                  size: 20,
                                  color: palette.miku,
                                )
                                : const Icon(
                                  Icons.chevron_right_rounded,
                                  size: 20,
                                ),
                        enabled:
                            !busySwitch &&
                            (!sessionEnded || activeProjectId == null),
                        onTap: onSelectGlobalScope,
                      ),
                    ),
                    const SizedBox(height: 18),
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(
                          Icons.link_rounded,
                          size: 17,
                          color: palette.muted,
                        ),
                        const SizedBox(width: 8),
                        Expanded(
                          child: Text(
                            '要加入資料夾，直接在對話中告訴 Miku 路徑；執行前仍會請你核准。',
                            style: Theme.of(context).textTheme.bodySmall
                                ?.copyWith(color: palette.muted),
                          ),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
            ),
          ],
        );
      },
    );
  }
}

class _ProjectCatalogCard extends StatelessWidget {
  const _ProjectCatalogCard({
    required this.project,
    required this.active,
    required this.loading,
    required this.enabled,
    required this.sessionEnded,
    required this.onTap,
    required this.onArchive,
  });

  final ProjectCatalogEntry project;
  final bool active;
  final bool loading;
  final bool enabled;
  final bool sessionEnded;
  final VoidCallback onTap;
  final VoidCallback? onArchive;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final folderCount = project.linkedFolderUris.length;
    final subtitle =
        active
            ? '目前對話正在這裡'
            : sessionEnded
            ? '請先開新對話'
            : folderCount == 0
            ? '可直接開始，不需要資料夾'
            : '$folderCount 個連結資料夾';
    return Semantics(
      button: true,
      selected: active,
      label: '${project.title} Project${active ? '，目前使用中' : ''}',
      child: ListTile(
        key: Key('project-${project.id}'),
        minTileHeight: 76,
        contentPadding: const EdgeInsets.fromLTRB(14, 8, 6, 8),
        selected: active,
        selectedTileColor: palette.miku.withValues(alpha: 0.09),
        tileColor: Theme.of(context).colorScheme.surface,
        shape: RoundedRectangleBorder(
          side: BorderSide(
            color:
                active ? palette.miku.withValues(alpha: 0.48) : palette.outline,
          ),
          borderRadius: BorderRadius.circular(18),
        ),
        leading: Container(
          width: 42,
          height: 42,
          decoration: BoxDecoration(
            color: palette.miku.withValues(alpha: active ? 0.16 : 0.08),
            borderRadius: BorderRadius.circular(13),
          ),
          child: Icon(
            project.hasLinkedFolder
                ? Icons.folder_copy_outlined
                : Icons.workspaces_outline,
            size: 21,
            color: active ? palette.miku : null,
          ),
        ),
        title: Text(
          project.title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(fontWeight: FontWeight.w600),
        ),
        subtitle: Padding(
          padding: const EdgeInsets.only(top: 3),
          child: Text(subtitle, maxLines: 1, overflow: TextOverflow.ellipsis),
        ),
        trailing:
            loading
                ? const Padding(
                  padding: EdgeInsets.only(right: 12),
                  child: SizedBox.square(
                    dimension: 18,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                )
                : _ProjectTrailing(
                  projectId: project.id,
                  active: active,
                  onArchive: onArchive,
                ),
        enabled: enabled,
        onTap: onTap,
      ),
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
          Icon(Icons.check_circle_rounded, size: 20, color: palette.miku)
        else
          const Icon(Icons.chevron_right_rounded, size: 20),
        PopupMenuButton<_ProjectMenuAction>(
          key: Key('project-archive-$projectId'),
          tooltip: 'Project 選項',
          enabled: onArchive != null,
          onSelected: (action) {
            if (action == _ProjectMenuAction.archive) onArchive?.call();
          },
          itemBuilder:
              (context) => const [
                PopupMenuItem(
                  value: _ProjectMenuAction.archive,
                  child: Row(
                    children: [
                      Icon(Icons.archive_outlined, size: 19),
                      SizedBox(width: 10),
                      Text('封存 Project'),
                    ],
                  ),
                ),
              ],
          icon: const Icon(Icons.more_horiz_rounded, size: 21),
        ),
      ],
    );
  }
}

enum _ProjectMenuAction { archive }

class _ProjectDirectoryView extends StatelessWidget {
  const _ProjectDirectoryView({
    required this.project,
    required this.path,
    required this.entries,
    required this.overview,
    required this.browserLoading,
    required this.previewingResourceUri,
    required this.error,
    required this.onRetry,
    required this.onOpenEntry,
    required this.onContinueConversation,
    required this.startingConversation,
    required this.onNewConversation,
  });

  final ProjectCatalogEntry project;
  final List<_ProjectBrowserLocation> path;
  final List<MikuResourceEntry>? entries;
  final ProjectOverview? overview;
  final bool browserLoading;
  final String? previewingResourceUri;
  final String? error;
  final VoidCallback onRetry;
  final ValueChanged<MikuResourceEntry> onOpenEntry;
  final VoidCallback onContinueConversation;
  final bool startingConversation;
  final VoidCallback onNewConversation;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final values = entries;
    final atRoot = path.length == 1;
    return Column(
      key: const Key('project-page-content'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (browserLoading) const LinearProgressIndicator(minHeight: 2),
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
                          if (atRoot && overview != null)
                            _ProjectOverviewSummary(
                              project: project,
                              overview: overview!,
                              onContinueConversation: onContinueConversation,
                              startingConversation: startingConversation,
                              onNewConversation: onNewConversation,
                            ),
                          if (error != null) ...[
                            const SizedBox(height: 12),
                            _DrawerErrorState(error: error!, onRetry: onRetry),
                          ],
                          if (atRoot) ...[
                            const SizedBox(height: 22),
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
                                  project.hasLinkedFolder
                                      ? '${project.linkedFolderUris.length} 個資料夾'
                                      : '尚未連結',
                                  style: Theme.of(context).textTheme.labelMedium
                                      ?.copyWith(color: palette.muted),
                                ),
                              ],
                            ),
                            const SizedBox(height: 4),
                            Text(
                              project.hasLinkedFolder
                                  ? '只顯示你明確授權給這個 Project 的檔案。'
                                  : '這個 Project 可以先用來規劃；需要檔案時再請 Miku 連結。',
                              style: Theme.of(context).textTheme.bodySmall
                                  ?.copyWith(color: palette.muted),
                            ),
                            const SizedBox(height: 10),
                          ],
                          if (browserLoading && values == null)
                            const _DrawerLoadingState(label: '讀取資料…')
                          else if (values == null)
                            const SizedBox.shrink()
                          else if (values.isEmpty)
                            _ProjectFilesEmptyState(
                              folderless: atRoot && !project.hasLinkedFolder,
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
                                          !browserLoading &&
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

class _ProjectOverviewSummary extends StatelessWidget {
  const _ProjectOverviewSummary({
    required this.project,
    required this.overview,
    required this.onContinueConversation,
    required this.startingConversation,
    required this.onNewConversation,
  });

  final ProjectCatalogEntry project;
  final ProjectOverview overview;
  final VoidCallback onContinueConversation;
  final bool startingConversation;
  final VoidCallback onNewConversation;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final actions =
        overview.nextActions
            .where((action) => action.trim().isNotEmpty)
            .take(3)
            .toList();
    final hasContext =
        overview.openLoops.isNotEmpty || overview.decisions.isNotEmpty;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          padding: const EdgeInsets.all(18),
          decoration: BoxDecoration(
            color: palette.miku.withValues(alpha: 0.09),
            border: Border.all(color: palette.miku.withValues(alpha: 0.30)),
            borderRadius: BorderRadius.circular(20),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
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
                    child: Icon(
                      Icons.auto_awesome_mosaic_outlined,
                      color: palette.miku,
                      size: 21,
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          project.title,
                          style: Theme.of(context).textTheme.titleLarge
                              ?.copyWith(fontWeight: FontWeight.w700),
                        ),
                        if (overview.status.trim().isNotEmpty)
                          Text(
                            overview.status,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: Theme.of(context).textTheme.bodySmall
                                ?.copyWith(color: palette.muted),
                          ),
                      ],
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 18),
              Text(
                actions.isEmpty ? '下一步還沒整理好' : '接下來',
                style: Theme.of(context).textTheme.labelLarge?.copyWith(
                  color: palette.miku,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 7),
              if (actions.isEmpty)
                Text(
                  '回到對話，和 Miku 決定一個最小的下一步。',
                  style: Theme.of(context).textTheme.bodyMedium,
                )
              else
                for (final action in actions)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Padding(
                          padding: const EdgeInsets.only(top: 6),
                          child: Container(
                            width: 6,
                            height: 6,
                            decoration: BoxDecoration(
                              color: palette.miku,
                              shape: BoxShape.circle,
                            ),
                          ),
                        ),
                        const SizedBox(width: 10),
                        Expanded(
                          child: Text(
                            action,
                            maxLines: 3,
                            overflow: TextOverflow.ellipsis,
                            style: Theme.of(context).textTheme.bodyMedium
                                ?.copyWith(fontWeight: FontWeight.w500),
                          ),
                        ),
                      ],
                    ),
                  ),
              const SizedBox(height: 12),
              FilledButton.icon(
                key: const Key('project-new-conversation'),
                onPressed: startingConversation ? null : onNewConversation,
                icon:
                    startingConversation
                        ? const SizedBox.square(
                          dimension: 17,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                        : const Icon(Icons.add_comment_outlined, size: 18),
                label: Text(startingConversation ? '建立中…' : '在這個 Project 新增對話'),
              ),
              const SizedBox(height: 4),
              TextButton.icon(
                key: const Key('project-continue-conversation'),
                onPressed: startingConversation ? null : onContinueConversation,
                icon: const Icon(Icons.arrow_back_rounded, size: 18),
                label: const Text('回到目前對話'),
              ),
            ],
          ),
        ),
        if (hasContext) ...[
          const SizedBox(height: 12),
          DecoratedBox(
            decoration: BoxDecoration(
              color: Theme.of(context).colorScheme.surface,
              border: Border.all(color: palette.outline),
              borderRadius: BorderRadius.circular(18),
            ),
            child: ExpansionTile(
              key: const Key('project-context-details'),
              tilePadding: const EdgeInsets.symmetric(horizontal: 16),
              childrenPadding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
              leading: Icon(
                Icons.layers_outlined,
                size: 21,
                color: palette.muted,
              ),
              title: const Text('Project 脈絡'),
              subtitle: Text(
                '${overview.openLoops.length} 個待處理 · ${overview.decisions.length} 個決定',
                style: TextStyle(color: palette.muted),
              ),
              children: [
                if (overview.openLoops.isNotEmpty)
                  _ProjectItemGroup(
                    label: '待處理',
                    icon: Icons.pending_actions_outlined,
                    items: overview.openLoops,
                  ),
                if (overview.openLoops.isNotEmpty &&
                    overview.decisions.isNotEmpty)
                  const SizedBox(height: 14),
                if (overview.decisions.isNotEmpty)
                  _ProjectItemGroup(
                    label: '已決定',
                    icon: Icons.rule_rounded,
                    items: overview.decisions,
                  ),
              ],
            ),
          ),
        ],
      ],
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
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Icon(icon, size: 17, color: palette.muted),
            const SizedBox(width: 7),
            Text(
              label,
              style: Theme.of(
                context,
              ).textTheme.labelLarge?.copyWith(color: palette.muted),
            ),
          ],
        ),
        const SizedBox(height: 7),
        for (final item in items.take(5))
          Padding(
            padding: const EdgeInsets.only(bottom: 6, left: 24),
            child: Text(
              item.text,
              maxLines: 3,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.bodyMedium,
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
