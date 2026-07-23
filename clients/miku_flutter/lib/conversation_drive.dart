part of 'conversation_screen.dart';

/// §30: Drive is Miku's playground — the durable space shared with Miku, not a project's folder.
/// The feed is scope-relative: a project session sees that project's shelf; a global session sees
/// the unprojected playground. The page owns its own load state so it survives push navigation.
class _DrivePage extends StatefulWidget {
  const _DrivePage({required this.client, required this.session});

  final MikuSessionClient client;
  final MikuSession session;

  @override
  State<_DrivePage> createState() => _DrivePageState();
}

class _DrivePageState extends State<_DrivePage> {
  DriveFeed? _feed;
  bool _loading = false;
  String? _error;
  String? _previewingUri;

  String? get _projectId => widget.session.projectId;

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
      final feed = await widget.client.driveFeed(
        widget.session.id,
        project: _projectId,
      );
      if (!mounted) return;
      setState(() => _feed = feed);
    } catch (_) {
      if (!mounted) return;
      setState(() => _error = '硬碟暫時讀不到，請再試一次。');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _openItem(DriveFeedItem item) async {
    if (_previewingUri != null) return;
    setState(() {
      _previewingUri = item.uri;
      _error = null;
    });
    try {
      final resource = await widget.client.previewResource(
        widget.session.id,
        item.uri,
      );
      if (!mounted) return;
      setState(() => _previewingUri = null);
      await showModalBottomSheet<void>(
        context: context,
        useSafeArea: true,
        isScrollControlled: true,
        showDragHandle: true,
        builder:
            (context) => _DriveDocumentSheet(item: item, resource: resource),
      );
    } catch (_) {
      if (!mounted) return;
      setState(() => _error = '這份硬碟文件暫時無法預覽。');
    } finally {
      if (mounted) setState(() => _previewingUri = null);
    }
  }

  Future<void> _openVirtualDir(DriveVirtualDir directory) async {
    if (_previewingUri != null) return;
    setState(() {
      _previewingUri = directory.uri;
      _error = null;
    });
    try {
      final resource = await widget.client.previewResource(
        widget.session.id,
        directory.uri,
      );
      if (!mounted) return;
      setState(() => _previewingUri = null);
      final item = DriveFeedItem(
        uri: directory.uri,
        path: directory.uri,
        title: directory.title,
      );
      await showModalBottomSheet<void>(
        context: context,
        useSafeArea: true,
        isScrollControlled: true,
        showDragHandle: true,
        builder:
            (context) => _DriveDocumentSheet(item: item, resource: resource),
      );
    } catch (_) {
      if (!mounted) return;
      setState(() => _error = '這份硬碟文件暫時無法預覽。');
    } finally {
      if (mounted) setState(() => _previewingUri = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    final project = _projectId;
    return Scaffold(
      appBar: AppBar(
        title: const Text('硬碟'),
        actions: [
          IconButton(
            key: const Key('drive-refresh'),
            tooltip: '重新整理硬碟',
            onPressed: _loading ? null : _load,
            icon: const Icon(Icons.refresh_rounded),
          ),
        ],
      ),
      body: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 12, 20, 6),
              child: Row(
                children: [
                  Icon(
                    Icons.folder_open_rounded,
                    size: 20,
                    color: palette.miku,
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      project == null ? 'Miku 的空間' : '$project 範圍',
                      style: Theme.of(context).textTheme.titleMedium?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
              child: Text(
                project == null
                    ? '你分享給 Miku、以及 Miku 收進來的內容都在這裡。'
                    : '這個專案範圍內分享給 Miku 的內容。',
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(color: palette.muted),
              ),
            ),
            if (_loading) const LinearProgressIndicator(minHeight: 2),
            Expanded(child: _buildBody(palette)),
          ],
        ),
      ),
    );
  }

  Widget _buildBody(TmTokens palette) {
    final feed = _feed;
    if (_loading && feed == null) {
      return const _DrawerLoadingState(label: '載入硬碟…');
    }
    if (_error != null && feed == null) {
      return _DrawerErrorState(error: _error!, onRetry: _load);
    }
    if (feed == null) return const SizedBox.shrink();
    if (feed.isEmpty) {
      return const _DrawerEmptyState(text: '硬碟還沒有內容。');
    }
    return ListView(
      key: const Key('drive-page-content'),
      padding: const EdgeInsets.fromLTRB(12, 4, 12, 20),
      children: [
        if (_error != null) ...[
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            child: _DriveInlineError(message: _error!, onRetry: _load),
          ),
        ],
        if (feed.recent.isNotEmpty) ...[
          const _DriveSectionLabel('最近文件'),
          for (final item in feed.recent)
            _DriveDocumentTile(
              item: item,
              loading: _previewingUri == item.uri,
              onTap: () => _openItem(item),
            ),
        ],
        if (feed.proposals.isNotEmpty) ...[
          const _DriveSectionLabel('整理建議'),
          for (final proposal in feed.proposals)
            _DriveProposalCard(proposal: proposal),
        ],
        if (feed.pendingApprovals.isNotEmpty) ...[
          const _DriveSectionLabel('等待確認'),
          for (final approval in feed.pendingApprovals)
            _DrivePendingApprovalCard(approval: approval),
        ],
        if (feed.virtualDirs.isNotEmpty) ...[
          const _DriveSectionLabel('檢視方式'),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final directory in feed.virtualDirs)
                  Tooltip(
                    message: directory.title,
                    child: ActionChip(
                      key: Key('drive-virtual-dir-${directory.uri}'),
                      avatar: const Icon(Icons.filter_alt_outlined, size: 16),
                      label: Text(_driveDirectoryLabel(directory)),
                      visualDensity: VisualDensity.compact,
                      onPressed: () => _openVirtualDir(directory),
                    ),
                  ),
              ],
            ),
          ),
        ],
      ],
    );
  }
}

class _DriveSectionLabel extends StatelessWidget {
  const _DriveSectionLabel(this.label);

  final String label;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 12, 8, 6),
      child: Text(
        label,
        style: Theme.of(context).textTheme.labelMedium?.copyWith(
          color: TmTokens.of(context).muted,
          fontWeight: FontWeight.w600,
        ),
      ),
    );
  }
}

class _DriveDocumentTile extends StatelessWidget {
  const _DriveDocumentTile({
    required this.item,
    required this.loading,
    required this.onTap,
  });

  final DriveFeedItem item;
  final bool loading;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    return Semantics(
      button: true,
      label: '${item.displayTitle}，硬碟文件',
      child: ListTile(
        key: Key('drive-document-${item.uri}'),
        dense: true,
        contentPadding: const EdgeInsets.symmetric(horizontal: 8),
        leading:
            loading
                ? const SizedBox.square(
                  dimension: 20,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
                : Icon(Icons.description_outlined, color: palette.miku),
        title: Text(
          item.displayTitle,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: Text(
          item.displayPreview,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
        ),
        trailing: const Icon(Icons.chevron_right_rounded),
        enabled: !loading,
        onTap: loading ? null : onTap,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
      ),
    );
  }
}

class _DriveProposalCard extends StatelessWidget {
  const _DriveProposalCard({required this.proposal});

  final DriveOrganizerProposal proposal;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    final confidence = proposal.confidence;
    return Container(
      key: Key('drive-proposal-${proposal.proposalId}'),
      margin: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: palette.approvalSurface,
        border: Border.all(color: palette.approvalOutline),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.auto_awesome_outlined, size: 18, color: palette.warm),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  proposal.displayTitle,
                  style: Theme.of(
                    context,
                  ).textTheme.labelLarge?.copyWith(fontWeight: FontWeight.w600),
                ),
              ),
              if (confidence != null)
                Text(
                  '${(confidence * 100).round()}%',
                  style: Theme.of(
                    context,
                  ).textTheme.labelSmall?.copyWith(color: palette.muted),
                ),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            proposal.displayPath,
            maxLines: 3,
            overflow: TextOverflow.ellipsis,
            style: Theme.of(context).textTheme.bodySmall,
          ),
          if (proposal.previewSnippet?.trim().isNotEmpty ?? false) ...[
            const SizedBox(height: 5),
            Text(
              proposal.previewSnippet!,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
          ],
        ],
      ),
    );
  }
}

class _DrivePendingApprovalCard extends StatelessWidget {
  const _DrivePendingApprovalCard({required this.approval});

  final DrivePendingApproval approval;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    return Container(
      key: Key('drive-pending-${approval.approvalId}'),
      margin: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        border: Border.all(color: palette.approvalOutline),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.verified_user_outlined, size: 18, color: palette.warm),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(approval.action),
                if (approval.preview?.trim().isNotEmpty ?? false)
                  Text(
                    approval.preview!,
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
                const SizedBox(height: 4),
                Text(
                  '回到對話卡片確認',
                  style: Theme.of(
                    context,
                  ).textTheme.labelSmall?.copyWith(color: palette.muted),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _DriveInlineError extends StatelessWidget {
  const _DriveInlineError({required this.message, required this.onRetry});

  final String message;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Expanded(
          child: Text(message, style: Theme.of(context).textTheme.bodySmall),
        ),
        TextButton(onPressed: onRetry, child: const Text('重試')),
      ],
    );
  }
}

class _DriveDocumentSheet extends StatelessWidget {
  const _DriveDocumentSheet({required this.item, required this.resource});

  final DriveFeedItem item;
  final ResourcePreview resource;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    final text =
        resource.preview.trim().isEmpty
            ? item.displayPreview
            : resource.preview;
    return FractionallySizedBox(
      heightFactor: 0.82,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              resource.title?.trim().isNotEmpty == true
                  ? resource.title!
                  : item.displayTitle,
              key: const Key('drive-preview-title'),
              style: Theme.of(
                context,
              ).textTheme.titleLarge?.copyWith(fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 4),
            Text(
              item.path,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                color: palette.muted,
                fontFamily: 'monospace',
              ),
            ),
            const SizedBox(height: 14),
            Expanded(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  border: Border.all(color: palette.outline),
                  borderRadius: BorderRadius.circular(14),
                ),
                child: SingleChildScrollView(
                  padding: const EdgeInsets.all(16),
                  child: SelectableText(
                    text,
                    key: const Key('drive-preview-content'),
                    style: Theme.of(context).textTheme.bodyMedium,
                  ),
                ),
              ),
            ),
            if (resource.hasMore) ...[
              const SizedBox(height: 10),
              Text(
                '這是伺服器提供的有界預覽。',
                key: const Key('drive-preview-truncated'),
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(color: palette.muted),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

String _driveDirectoryLabel(DriveVirtualDir directory) {
  return switch (directory.name) {
    'recent' => '最近',
    'by-project' => '專案',
    'by-type' => '類型',
    'by-tag' => '標籤',
    'by-date' => '日期',
    _ => directory.title.isEmpty ? directory.name : directory.title,
  };
}
