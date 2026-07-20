part of 'conversation_screen.dart';

/// Full-page session history. §30.6: promotion is demoted to session assignment — declaring that a
/// (usually closed) session belongs to a project. This page lists sessions, opens one, and assigns
/// one to a project entity; the server re-runs observation extraction so project items grow through
/// the one pipeline regardless of when assignment happened.
class _HistoryPage extends StatefulWidget {
  const _HistoryPage({
    required this.client,
    required this.currentSessionId,
    required this.onSelectSession,
  });

  final MikuSessionClient client;
  final String? currentSessionId;
  final ValueChanged<String> onSelectSession;

  @override
  State<_HistoryPage> createState() => _HistoryPageState();
}

class _HistoryPageState extends State<_HistoryPage> {
  List<SessionSummary>? _sessions;
  List<ProjectCatalogEntry>? _projects;
  bool _loading = false;
  String? _error;
  String? _assigningSessionId;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    if (_loading) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final sessions = await widget.client.listSessions(limit: 30);
      if (!mounted) return;
      setState(() => _sessions = sessions);
      // Best-effort project catalog for the assignment picker; failure leaves assignment disabled.
      try {
        final projects = await widget.client.listProjects();
        if (mounted) setState(() => _projects = projects);
      } catch (_) {
        /* assignment picker stays unavailable */
      }
    } catch (_) {
      if (!mounted) return;
      setState(() => _error = 'History 暫時讀不到，請再試一次。');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _assign(SessionSummary session) async {
    final projects =
        _projects?.where((project) => project.status == 'active').toList();
    if (projects == null || projects.isEmpty) {
      _notify('目前沒有可指派的 Project，先建立一個。');
      return;
    }
    final project = await showModalBottomSheet<ProjectCatalogEntry>(
      context: context,
      showDragHandle: true,
      builder:
          (context) =>
              _AssignProjectSheet(session: session, projects: projects),
    );
    if (project == null || !mounted) return;
    setState(() {
      _assigningSessionId = session.id;
      _error = null;
    });
    try {
      final grown = await widget.client.assignSessionToProject(
        project.id,
        session.id,
      );
      if (!mounted) return;
      _notify('已指派到 ${project.title}；成長了 $grown 個 Project 項目。');
    } catch (_) {
      if (!mounted) return;
      _notify('指派失敗，請稍後再試。');
    } finally {
      if (mounted) setState(() => _assigningSessionId = null);
    }
  }

  void _notify(String message) {
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('History'),
        actions: [
          IconButton(
            tooltip: '重新整理 History',
            onPressed: _loading ? null : _load,
            icon: const Icon(Icons.refresh_rounded),
          ),
        ],
      ),
      body: SafeArea(child: _buildBody()),
    );
  }

  Widget _buildBody() {
    final sessions = _sessions;
    if (_loading && sessions == null) {
      return const _DrawerLoadingState(label: '載入 History…');
    }
    if (_error != null && sessions == null) {
      return _DrawerErrorState(error: _error!, onRetry: _load);
    }
    if (sessions == null) return const SizedBox.shrink();
    if (sessions.isEmpty) {
      return const _DrawerEmptyState(text: '還沒有對話紀錄。');
    }
    final palette = _Palette.of(context);
    final canAssign = (_projects?.isNotEmpty ?? false);
    return ListView(
      key: const Key('history-page-content'),
      padding: const EdgeInsets.fromLTRB(12, 12, 12, 20),
      children: [
        if (_error != null)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: _DriveInlineError(message: _error!, onRetry: _load),
          ),
        for (final session in sessions)
          Padding(
            padding: const EdgeInsets.only(bottom: 4),
            child: ListTile(
              key: Key('history-session-${session.id}'),
              minTileHeight: 56,
              selected: session.id == widget.currentSessionId,
              selectedTileColor: palette.miku.withValues(alpha: 0.10),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(10),
              ),
              title: Text(
                session.title.trim().isEmpty ? '新對話' : session.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              subtitle: Text(
                session.preview.trim().isEmpty
                    ? session.label
                    : session.preview,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              trailing: _HistoryTrailing(
                sessionId: session.id,
                current: session.id == widget.currentSessionId,
                assigning: _assigningSessionId == session.id,
                assignable: session.status == 'ended',
                canAssign: canAssign && _assigningSessionId == null,
                onAssign: () => _assign(session),
              ),
              onTap: () => widget.onSelectSession(session.id),
            ),
          ),
      ],
    );
  }
}

class _HistoryTrailing extends StatelessWidget {
  const _HistoryTrailing({
    required this.sessionId,
    required this.current,
    required this.assigning,
    required this.assignable,
    required this.canAssign,
    required this.onAssign,
  });

  final String sessionId;
  final bool current;
  final bool assigning;
  final bool assignable;
  final bool canAssign;
  final VoidCallback onAssign;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (current) Icon(Icons.check_rounded, size: 18, color: palette.miku),
        if (assigning)
          const SizedBox.square(
            dimension: 18,
            child: CircularProgressIndicator(strokeWidth: 2),
          )
        else if (assignable)
          IconButton(
            key: Key('history-assign-$sessionId'),
            tooltip: '指派到 Project',
            visualDensity: VisualDensity.compact,
            onPressed: canAssign ? onAssign : null,
            icon: const Icon(Icons.drive_file_move_outline, size: 19),
          ),
      ],
    );
  }
}

class _AssignProjectSheet extends StatelessWidget {
  const _AssignProjectSheet({required this.session, required this.projects});

  final SessionSummary session;
  final List<ProjectCatalogEntry> projects;

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(20, 4, 20, 8),
            child: Text(
              '指派「${session.title.trim().isEmpty ? '新對話' : session.title}」到 Project',
              style: Theme.of(
                context,
              ).textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w600),
            ),
          ),
          Flexible(
            child: ListView(
              shrinkWrap: true,
              children: [
                for (final project in projects)
                  ListTile(
                    key: Key('assign-project-${project.id}'),
                    leading: Icon(
                      project.hasLinkedFolder
                          ? Icons.folder_copy_outlined
                          : Icons.workspaces_outline,
                    ),
                    title: Text(project.title),
                    subtitle: Text(project.hasLinkedFolder ? '已連結資料夾' : '規劃用'),
                    onTap: () => Navigator.of(context).pop(project),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
