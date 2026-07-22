part of 'conversation_screen.dart';

/// §30: A project is a first-class, server-owned entity with a subject — not a linked-folder alias.
/// A folder is an optional 0..n attachment; a project may be planning-only with no folder. This page
/// is entity-first: it lists project entities plus a pinned Global scope, opens a per-scope detail
/// with 記憶 / 檔案 / 脈絡 tabs, assigns the current session to a scope, and browses attached linked
/// folders. Linking a folder is a Miku-mediated approval-gated host call (§30.3), surfaced here as
/// guidance rather than a direct client action.
///
/// The 記憶 tab reflects the session's active memory scope (§22.6): project recall when the policy is
/// `project`, global recall otherwise. Memory reads are authority-checked server-side against that
/// exact active scope, so switching the policy switches what this surface can show — and archived or
/// revoked scopes fail closed rather than leaking.
class _ProjectPage extends StatefulWidget {
  const _ProjectPage({
    required this.client,
    required this.session,
    required this.sessionEnded,
    required this.onMemoryContextChanged,
    required this.onNewConversation,
  });

  final MikuSessionClient client;
  final MikuSession session;
  final bool sessionEnded;

  /// Reports a committed project and memory-policy change back to the conversation.
  final void Function(String? projectId, MikuMemoryPolicy memoryPolicy)
  onMemoryContextChanged;

  final Future<bool> Function(ProjectCatalogEntry project) onNewConversation;

  @override
  State<_ProjectPage> createState() => _ProjectPageState();
}

/// Sentinel root location for the Global scope detail (Global owns no linked folders).
const String _globalRootUri = 'scope://global';

class _ProjectPageState extends State<_ProjectPage> {
  List<ProjectCatalogEntry>? _projects;
  ProjectOverview? _overview;
  List<MikuResourceEntry>? _entries;
  final List<_ProjectBrowserLocation> _path = [];
  String? _activeProjectId;
  MikuMemoryPolicy _memoryPolicy = MikuMemoryPolicy.global;

  // Memory surface for the active scope (§22): dream summaries + scoped recall chunks.
  List<MikuResourceEntry>? _memorySummaries;
  List<MikuResourceEntry>? _memoryChunks;
  bool _memoryLoading = false;
  String? _memoryError;

  bool _catalogLoading = false;
  bool _browserLoading = false;
  String? _switchingProjectId;
  bool _switchingToGlobal = false;
  String? _previewingUri;
  MikuResourceEntry? _failedPreviewEntry;
  String? _error;
  bool _startingConversation = false;
  bool _busy = false;

  /// Tab to restore when returning to a scope root from a drilled folder (0=記憶, 1=檔案, 2=脈絡).
  int _rootTabIndex = 0;

  bool get _inDetail => _path.isNotEmpty;
  bool get _atRoot => _path.length == 1;
  bool get _isGlobalDetail => _inDetail && _activeProjectId == null;

  /// The active memory scope string, matching the server's `session.memory_scope()` (§22.6).
  String get _activeScope =>
      _memoryPolicy == MikuMemoryPolicy.project && _activeProjectId != null
          ? 'project:${_activeProjectId!}'
          : 'global';

  @override
  void initState() {
    super.initState();
    _activeProjectId = widget.session.projectId;
    _memoryPolicy = widget.session.memoryPolicy;
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
      // Deep-link into the active project's detail so the current conversation's
      // memory is front and centre; a folderless/no-project session lands on the
      // scope catalog, where Global sits as a pinned card.
      if (_activeProjectId != null) {
        final active =
            catalog.where((item) => item.id == _activeProjectId).firstOrNull;
        if (active != null) await _loadRoot(active);
      }
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _catalogLoading = false);
    }
  }

  Future<void> _openProject(ProjectCatalogEntry project) async {
    if (_switchingProjectId != null || _switchingToGlobal) return;
    if (widget.sessionEnded && project.id != _activeProjectId) return;
    if (project.id == _activeProjectId) {
      _rootTabIndex = 0;
      await _loadRoot(project);
      return;
    }
    setState(() {
      _switchingProjectId = project.id;
      _error = null;
    });
    try {
      final updated = await widget.client.setSessionMemoryContext(
        widget.session.id,
        projectId: project.id,
        memoryPolicy: project.defaultMemoryPolicy,
      );
      if (!mounted) return;
      if (updated.projectId != project.id) {
        throw StateError(
          'server selected unexpected project ${updated.projectId}',
        );
      }
      widget.onMemoryContextChanged(updated.projectId, updated.memoryPolicy);
      setState(() {
        _activeProjectId = updated.projectId;
        _memoryPolicy = updated.memoryPolicy;
        _overview = null;
        _entries = null;
        _rootTabIndex = 0;
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

  Future<void> _openGlobal() async {
    if (_switchingProjectId != null || _switchingToGlobal) return;
    if (widget.sessionEnded && _activeProjectId != null) return;
    if (_activeProjectId == null) {
      _enterGlobalRoot();
      await _loadMemory();
      return;
    }
    setState(() {
      _switchingToGlobal = true;
      _error = null;
    });
    try {
      final updated = await widget.client.setSessionMemoryContext(
        widget.session.id,
        projectId: null,
        memoryPolicy: MikuMemoryPolicy.global,
      );
      if (!mounted) return;
      widget.onMemoryContextChanged(updated.projectId, updated.memoryPolicy);
      setState(() {
        _activeProjectId = updated.projectId;
        _memoryPolicy = updated.memoryPolicy;
        _overview = null;
        _entries = null;
      });
      _enterGlobalRoot();
      await _loadMemory();
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _switchingToGlobal = false);
    }
  }

  void _enterGlobalRoot() {
    setState(() {
      _rootTabIndex = 0;
      _path
        ..clear()
        ..add(
          const _ProjectBrowserLocation(uri: _globalRootUri, label: 'Global'),
        );
    });
  }

  Future<void> _setMemoryPolicy(MikuMemoryPolicy policy) async {
    if (_activeProjectId == null || _busy || policy == _memoryPolicy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final updated = await widget.client.setSessionMemoryContext(
        widget.session.id,
        projectId: _activeProjectId,
        memoryPolicy: policy,
      );
      if (!mounted) return;
      widget.onMemoryContextChanged(updated.projectId, updated.memoryPolicy);
      setState(() => _memoryPolicy = updated.memoryPolicy);
      await _loadMemory();
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _busy = false);
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
    await _openProject(created);
  }

  Future<void> _archiveProject(ProjectCatalogEntry project) async {
    if (_busy) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder:
          (context) => AlertDialog(
            title: Text('封存 ${project.title}？'),
            content: const Text(
              '封存後這個 Project 不會出現在選單，它的記憶範圍會停用：之後不再寫入，也不會被回想。已保留的記錄仍留作歷史，不會刪除。這是唯一會停用記憶範圍的動作。',
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
        await _openGlobal();
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
      await _loadMemory();
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _friendlyProjectError(error));
    } finally {
      if (mounted) setState(() => _browserLoading = false);
    }
  }

  Future<void> _loadMemory() async {
    final scope = _activeScope;
    setState(() {
      _memoryLoading = true;
      _memoryError = null;
    });
    // §22.3: recall degrades gracefully — a single store being down or empty
    // must not blank the whole surface, so fetch the two independently and only
    // surface an error when both fail.
    final results = await Future.wait([
      _tryListMemory('memory://scopes/$scope/chunks'),
      _tryListMemory('memory://summaries'),
    ]);
    if (!mounted || _activeScope != scope) return;
    final chunks = results[0];
    final summaries = results[1];
    setState(() {
      _memoryChunks = chunks.entries ?? const [];
      _memorySummaries = summaries.entries ?? const [];
      _memoryError =
          (chunks.entries == null && summaries.entries == null)
              ? _friendlyMemoryError(chunks.error ?? summaries.error!)
              : null;
      _memoryLoading = false;
    });
  }

  Future<_MemoryFetch> _tryListMemory(String uri) async {
    try {
      final entries = await widget.client.listResources(widget.session.id, uri);
      return _MemoryFetch(entries: entries);
    } catch (error) {
      return _MemoryFetch(error: error);
    }
  }

  Future<void> _openEntry(MikuResourceEntry entry) async {
    if (entry.isDirectory) {
      _rootTabIndex = 1;
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
        _memoryChunks = null;
        _memorySummaries = null;
        _memoryError = null;
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

  Future<void> _openMemoryEntry(MikuResourceEntry entry) async {
    if (_previewingUri != null) return;
    setState(() => _previewingUri = entry.uri);
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
      setState(() => _memoryError = _friendlyMemoryError(error));
    } finally {
      if (mounted) setState(() => _previewingUri = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: !_inDetail,
      onPopInvokedWithResult: (didPop, result) {
        if (!didPop) unawaited(_goUp());
      },
      child: Scaffold(
        appBar: AppBar(
          leading:
              !_inDetail
                  ? const BackButton()
                  : BackButton(
                    key: const Key('project-browser-up'),
                    onPressed: _browserLoading ? null : _goUp,
                  ),
          title: Text(_inDetail ? _path.last.label : 'Projects'),
          actions: [
            if (!_inDetail)
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
    if (!_inDetail) {
      return _ProjectCatalogList(
        projects: projects,
        activeProjectId: _activeProjectId,
        switchingProjectId: _switchingProjectId,
        switchingToGlobal: _switchingToGlobal,
        sessionEnded: widget.sessionEnded,
        busy: _busy,
        error: _error,
        onRetry: _loadCatalog,
        onOpenGlobal: _openGlobal,
        onOpen: _openProject,
        onArchive: _archiveProject,
      );
    }
    if (_isGlobalDetail) {
      return _ScopeDetailView(
        title: 'Global',
        subtitle: '所有沒有指定 Project 的對話共用這裡的記憶。',
        icon: Icons.public_rounded,
        isGlobal: true,
        atRoot: _atRoot,
        initialTabIndex: _rootTabIndex,
        onTabChanged: (index) => _rootTabIndex = index,
        memory: _buildMemorySection(),
        files: null,
        context_: null,
        onNewConversation: null,
        onContinueConversation: _continueConversation,
        startingConversation: _startingConversation,
      );
    }
    final activeProject =
        projects.where((project) => project.id == _activeProjectId).firstOrNull;
    if (activeProject == null) {
      return _DrawerErrorState(error: '找不到這個 Project。', onRetry: _loadCatalog);
    }
    return _ScopeDetailView(
      title: activeProject.title,
      subtitle:
          _overview?.status.trim().isNotEmpty == true
              ? _overview!.status
              : (activeProject.hasLinkedFolder
                  ? '${activeProject.linkedFolderUris.length} 個連結資料夾'
                  : '規劃中，尚未連結資料夾'),
      icon:
          activeProject.hasLinkedFolder
              ? Icons.folder_copy_outlined
              : Icons.workspaces_outline,
      isGlobal: false,
      atRoot: _atRoot,
      initialTabIndex: _rootTabIndex,
      onTabChanged: (index) => _rootTabIndex = index,
      memory: _buildMemorySection(),
      files: _buildFilesSection(activeProject),
      context_: _buildContextSection(),
      onNewConversation: () => _startConversation(activeProject),
      onContinueConversation: _continueConversation,
      startingConversation: _startingConversation,
    );
  }

  Widget _buildMemorySection() {
    return _ScopeMemoryTab(
      scope: _activeScope,
      policy: _memoryPolicy,
      canTogglePolicy: _activeProjectId != null && !widget.sessionEnded,
      policyBusy: _busy,
      onPolicyChanged: _setMemoryPolicy,
      summaries: _memorySummaries,
      chunks: _memoryChunks,
      loading: _memoryLoading,
      error: _memoryError,
      previewingUri: _previewingUri,
      onRetry: _loadMemory,
      onOpen: _openMemoryEntry,
    );
  }

  Widget _buildFilesSection(ProjectCatalogEntry project) {
    if (_atRoot) {
      return _ScopeFilesTab(
        hasLinkedFolder: project.hasLinkedFolder,
        folderCount: project.linkedFolderUris.length,
        entries: _entries,
        loading: _browserLoading,
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
      );
    }
    // Drilled into a subfolder: a plain listing occupies the whole detail body.
    return _ScopeFilesTab(
      hasLinkedFolder: true,
      folderCount: 0,
      entries: _entries,
      loading: _browserLoading,
      previewingResourceUri: _previewingUri,
      error: _error,
      hideHeader: true,
      onRetry: () {
        final failed = _failedPreviewEntry;
        if (failed != null) {
          unawaited(_openFile(failed));
          return;
        }
        unawaited(_loadLocation(_path.last, push: false));
      },
      onOpenEntry: _openEntry,
    );
  }

  Widget _buildContextSection() {
    return _ScopeContextTab(overview: _overview);
  }
}

class _ProjectBrowserLocation {
  const _ProjectBrowserLocation({required this.uri, required this.label});

  final String uri;
  final String label;
}

/// Result of one memory-store list, so the 記憶 tab degrades per store (§22.3).
class _MemoryFetch {
  const _MemoryFetch({this.entries, this.error});

  final List<MikuResourceEntry>? entries;
  final Object? error;
}

class _CreateProjectDialog extends StatefulWidget {
  const _CreateProjectDialog({required this.client});

  final MikuSessionClient client;

  @override
  State<_CreateProjectDialog> createState() => _CreateProjectDialogState();
}

class _CreateProjectDialogState extends State<_CreateProjectDialog> {
  final TextEditingController _controller = TextEditingController();
  MikuMemoryPolicy _defaultMemoryPolicy = MikuMemoryPolicy.project;
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
      final created = await widget.client.createProject(
        slug,
        title: title,
        defaultMemoryPolicy: _defaultMemoryPolicy,
      );
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
          SwitchListTile.adaptive(
            key: const Key('create-project-memory-policy'),
            contentPadding: EdgeInsets.zero,
            title: const Text('這個 Project 預設使用它自己的記憶'),
            value: _defaultMemoryPolicy == MikuMemoryPolicy.project,
            onChanged:
                _submitting
                    ? null
                    : (value) => setState(
                      () =>
                          _defaultMemoryPolicy =
                              value
                                  ? MikuMemoryPolicy.project
                                  : MikuMemoryPolicy.global,
                    ),
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
    required this.onOpenGlobal,
    required this.onOpen,
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
  final VoidCallback onOpenGlobal;
  final ValueChanged<ProjectCatalogEntry> onOpen;
  final ValueChanged<ProjectCatalogEntry> onArchive;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final busySwitch = switchingProjectId != null || switchingToGlobal || busy;
    return ListView(
      key: const Key('project-page-content'),
      padding: const EdgeInsets.fromLTRB(20, 12, 20, 28),
      children: [
        Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 760),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  '把對話放回它正在前進的事情裡。',
                  style: Theme.of(
                    context,
                  ).textTheme.titleLarge?.copyWith(fontWeight: FontWeight.w700),
                ),
                const SizedBox(height: 6),
                Text(
                  '選一個範圍，看看 Miku 為它記得什麼、連結了哪些資料。',
                  style: Theme.of(
                    context,
                  ).textTheme.bodyMedium?.copyWith(color: palette.muted),
                ),
                if (error != null) ...[
                  const SizedBox(height: 12),
                  _DrawerErrorState(error: error!, onRetry: onRetry),
                ],
                const SizedBox(height: 20),
                _GlobalScopeCard(
                  active: activeProjectId == null,
                  loading: switchingToGlobal,
                  enabled:
                      !busySwitch && (!sessionEnded || activeProjectId == null),
                  onTap: onOpenGlobal,
                ),
                const SizedBox(height: 18),
                Text(
                  '你的 Projects',
                  style: Theme.of(
                    context,
                  ).textTheme.labelLarge?.copyWith(color: palette.muted),
                ),
                const SizedBox(height: 8),
                if (projects.isEmpty)
                  const _DrawerEmptyState(text: '還沒有 Project。按右上角的「＋」建立第一個。')
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
                      onTap: () => onOpen(project),
                      onArchive: busySwitch ? null : () => onArchive(project),
                    ),
                    const SizedBox(height: 10),
                  ],
                const SizedBox(height: 18),
                Text(
                  '要加入資料夾，直接在對話中告訴 Miku 路徑；執行前仍會請你核准。',
                  style: Theme.of(
                    context,
                  ).textTheme.bodySmall?.copyWith(color: palette.muted),
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class _GlobalScopeCard extends StatelessWidget {
  const _GlobalScopeCard({
    required this.active,
    required this.loading,
    required this.enabled,
    required this.onTap,
  });

  final bool active;
  final bool loading;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      button: true,
      selected: active,
      label: 'Global 記憶範圍${active ? '，目前使用中' : ''}',
      child: ListTile(
        key: const Key('project-global-scope'),
        minTileHeight: 66,
        contentPadding: const EdgeInsets.fromLTRB(14, 8, 10, 8),
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
            Icons.public_rounded,
            size: 21,
            color: active ? palette.miku : null,
          ),
        ),
        title: const Text(
          'Global',
          style: TextStyle(fontWeight: FontWeight.w600),
        ),
        subtitle: Text(active ? '目前對話使用全域記憶' : '不綁定 Project 的全域記憶'),
        trailing:
            loading
                ? const SizedBox.square(
                  dimension: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
                : active
                ? Icon(
                  Icons.check_circle_rounded,
                  size: 20,
                  color: palette.miku,
                )
                : const Icon(Icons.chevron_right_rounded, size: 20),
        enabled: enabled,
        onTap: onTap,
      ),
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

String _friendlyMemoryError(Object error) {
  final message = error.toString();
  if (message.contains('404') ||
      message.contains('not found') ||
      message.contains('active project')) {
    return '這個範圍的記憶目前不可用（可能已封存或改用全域）。';
  }
  return '記憶暫時讀不到，請再試一次。';
}

String _formatBytes(int bytes) {
  if (bytes < 1024) return '$bytes B';
  if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KB';
  return '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MB';
}
