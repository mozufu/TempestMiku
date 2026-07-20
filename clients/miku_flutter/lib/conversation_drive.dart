part of 'conversation_screen.dart';

class _DriveDrawerContent extends StatelessWidget {
  const _DriveDrawerContent({
    required this.loading,
    required this.feed,
    required this.error,
    required this.activeProjectId,
    required this.previewingUri,
    required this.onRetry,
    required this.onOpenItem,
  });

  final bool loading;
  final DriveFeed? feed;
  final String? error;
  final String? activeProjectId;
  final String? previewingUri;
  final VoidCallback onRetry;
  final ValueChanged<DriveFeedItem> onOpenItem;

  @override
  Widget build(BuildContext context) {
    if (activeProjectId == null) {
      return const _DriveScopeState();
    }
    if (loading && feed == null) {
      return const _DrawerLoadingState(label: '載入 Drive…');
    }
    if (error != null && feed == null) {
      return _DrawerErrorState(error: error!, onRetry: onRetry);
    }
    final current = feed;
    if (current == null) return const SizedBox.shrink();

    return Padding(
      key: const Key('drawer-drive-content'),
      padding: const EdgeInsets.fromLTRB(12, 2, 8, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  activeProjectId!,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.labelMedium?.copyWith(
                    color: _Palette.of(context).muted,
                  ),
                ),
              ),
              IconButton(
                key: const Key('drive-refresh'),
                tooltip: '重新整理 Drive',
                onPressed: loading ? null : onRetry,
                icon: const Icon(Icons.refresh_rounded, size: 19),
              ),
            ],
          ),
          if (loading) const LinearProgressIndicator(minHeight: 2),
          if (error != null) ...[
            const SizedBox(height: 8),
            _DriveInlineError(message: error!, onRetry: onRetry),
          ],
          if (current.isEmpty)
            const _DrawerEmptyState(text: '這個 Project 的 Drive 還沒有內容。')
          else ...[
            if (current.recent.isNotEmpty) ...[
              const _DriveSectionLabel('最近文件'),
              for (final item in current.recent)
                _DriveDocumentTile(
                  item: item,
                  loading: previewingUri == item.uri,
                  onTap: () => onOpenItem(item),
                ),
            ],
            if (current.proposals.isNotEmpty) ...[
              const _DriveSectionLabel('整理建議'),
              for (final proposal in current.proposals)
                _DriveProposalCard(proposal: proposal),
            ],
            if (current.pendingApprovals.isNotEmpty) ...[
              const _DriveSectionLabel('等待確認'),
              for (final approval in current.pendingApprovals)
                _DrivePendingApprovalCard(approval: approval),
            ],
            if (current.virtualDirs.isNotEmpty) ...[
              const _DriveSectionLabel('檢視方式'),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final directory in current.virtualDirs)
                    Tooltip(
                      message: directory.title,
                      child: Chip(
                        avatar: const Icon(Icons.filter_alt_outlined, size: 16),
                        label: Text(_driveDirectoryLabel(directory)),
                        visualDensity: VisualDensity.compact,
                      ),
                    ),
                ],
              ),
            ],
          ],
        ],
      ),
    );
  }
}

class _DriveScopeState extends StatelessWidget {
  const _DriveScopeState();

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Padding(
      key: const Key('drive-project-required'),
      padding: const EdgeInsets.fromLTRB(20, 8, 12, 18),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.workspaces_outline, size: 18, color: palette.muted),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              '先在 Project 選擇工作範圍，Drive 只會顯示該範圍已授權的內容。',
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
          ),
        ],
      ),
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
          color: _Palette.of(context).muted,
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
    final palette = _Palette.of(context);
    return Semantics(
      button: true,
      label: '${item.displayTitle}，Drive 文件',
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
    final palette = _Palette.of(context);
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
    final palette = _Palette.of(context);
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
    final palette = _Palette.of(context);
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
    'by-project' => 'Project',
    'by-type' => '類型',
    'by-tag' => '標籤',
    'by-date' => '日期',
    _ => directory.title.isEmpty ? directory.name : directory.title,
  };
}
