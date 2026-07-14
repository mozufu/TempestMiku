part of 'main.dart';

class _DriveFeedSheet extends StatefulWidget {
  const _DriveFeedSheet({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.initialFeed,
    required this.initialError,
    required this.initialLoading,
    required this.approvals,
    required this.loadFeed,
    required this.onOpenResource,
    required this.onOpenApproval,
    this.embedded = false,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final DriveFeed? initialFeed;
  final String initialError;
  final bool initialLoading;
  final List<ApprovalPrompt> approvals;
  final Future<DriveFeed> Function() loadFeed;
  final void Function(String uri) onOpenResource;
  final void Function(ApprovalPrompt approval) onOpenApproval;
  final bool embedded;

  @override
  State<_DriveFeedSheet> createState() => _DriveFeedSheetState();
}

class _DriveFeedSheetState extends State<_DriveFeedSheet> {
  DriveFeed? _feed;
  Object? _error;
  bool _loading = false;
  int _refreshGeneration = 0;

  @override
  void initState() {
    super.initState();
    _feed = widget.initialFeed;
    _error = widget.initialError.isEmpty ? null : widget.initialError;
    _loading = widget.initialLoading || widget.initialFeed == null;
    unawaited(_refresh(silent: widget.initialFeed != null));
  }

  @override
  void didUpdateWidget(covariant _DriveFeedSheet oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.initialFeed == oldWidget.initialFeed &&
        widget.initialError == oldWidget.initialError) {
      return;
    }
    _refreshGeneration += 1;
    _feed = widget.initialFeed;
    _error = widget.initialError.isEmpty ? null : widget.initialError;
    if (widget.initialFeed == null && !_loading) {
      _loading = true;
      unawaited(_refresh(silent: true));
    } else if (widget.initialFeed != null) {
      _loading = false;
    }
  }

  Future<void> _refresh({bool silent = false}) async {
    final generation = ++_refreshGeneration;
    if (!silent && mounted) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final feed = await widget.loadFeed();
      if (!mounted || generation != _refreshGeneration) return;
      setState(() {
        _feed = feed;
        _loading = false;
        _error = null;
      });
    } catch (err) {
      if (!mounted || generation != _refreshGeneration) return;
      setState(() {
        _loading = false;
        _error = err;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final copy = widget.copy;
    final feed = _feed;
    final hasPendingDriveApprovals =
        widget.approvals.isNotEmpty ||
        (feed?.pendingApprovals.isNotEmpty ?? false);
    final showEmptyFeed =
        feed == null || (feed.isEmpty && !hasPendingDriveApprovals);
    final displayFeed = feed ?? DriveFeed.empty;
    return ListView(
      padding:
          widget.embedded
              ? const EdgeInsets.fromLTRB(20, 20, 20, 24)
              : const EdgeInsets.fromLTRB(15, 9, 15, 18),
      children: [
        if (!widget.embedded) ...[
          Center(
            child: Container(
              width: 38,
              height: 5,
              decoration: BoxDecoration(
                color: tok.border,
                borderRadius: BorderRadius.circular(999),
              ),
            ),
          ),
          const SizedBox(height: 14),
        ],
        Row(
          children: [
            Container(
              width: 38,
              height: 38,
              decoration: BoxDecoration(
                color: widget.accent,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Icon(
                Icons.folder_outlined,
                color: _textOn(widget.accent),
                size: 20,
              ),
            ),
            const SizedBox(width: 11),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    copy.driveFeed,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 17,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    copy.driveFeedHelper,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 12,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
              ),
            ),
            _TokIconBtn(
              tok: tok,
              icon: Icons.refresh,
              tooltip: copy.refresh,
              semanticLabel: copy.refreshDrive,
              onTap: _loading ? null : () => _refresh(),
            ),
            if (!widget.embedded) ...[
              const SizedBox(width: 8),
              _TokIconBtn(
                tok: tok,
                icon: Icons.close,
                tooltip: copy.close,
                semanticLabel: copy.closeDriveFeed,
                onTap: () => Navigator.pop(context),
              ),
            ],
          ],
        ),
        const SizedBox(height: 13),
        if (_loading && feed == null)
          _HistoryStateLine(
            tok: tok,
            icon: Icons.hourglass_top,
            text: copy.loadingDriveFeed,
          )
        else if (_error != null && feed == null)
          _HistoryStateLine(
            tok: tok,
            icon: Icons.error_outline,
            text: copy.driveFeedLoadFailed(_error!),
          )
        else if (showEmptyFeed)
          _HistoryStateLine(
            tok: tok,
            icon: Icons.folder_open_outlined,
            text: copy.noDriveFeed,
          )
        else ...[
          if (hasPendingDriveApprovals)
            _DriveSection(
              tok: tok,
              title: copy.pendingDriveApprovals,
              detail:
                  '${widget.approvals.length + displayFeed.pendingApprovals.length}',
              children: [
                for (final approval in widget.approvals)
                  _DriveApprovalRow(
                    tok: tok,
                    copy: copy,
                    approval: approval,
                    onTap: () => widget.onOpenApproval(approval),
                  ),
                for (final approval in displayFeed.pendingApprovals)
                  _DrivePendingApprovalRow(tok: tok, approval: approval),
              ],
            ),
          _DriveSection(
            tok: tok,
            title: copy.recentDocuments,
            detail: copy.driveDocs(displayFeed.recent.length),
            children:
                displayFeed.recent.isEmpty
                    ? [
                      _DriveEmptyLine(
                        tok: tok,
                        icon: Icons.folder_open_outlined,
                        text: copy.noDriveFeed,
                      ),
                    ]
                    : [
                      for (final item in displayFeed.recent)
                        _DriveFeedDocRow(
                          tok: tok,
                          copy: copy,
                          accent: widget.accent,
                          item: item,
                          onOpen: () => widget.onOpenResource(item.uri),
                        ),
                    ],
          ),
          _DriveSection(
            tok: tok,
            title: copy.virtualDirs,
            detail: '${displayFeed.virtualDirs.length}',
            children: [
              Wrap(
                spacing: 7,
                runSpacing: 7,
                children: [
                  for (final dir in displayFeed.virtualDirs)
                    _DriveVirtualDirChip(
                      tok: tok,
                      dir: dir,
                      onOpen: () => widget.onOpenResource(dir.uri),
                    ),
                ],
              ),
            ],
          ),
          if (displayFeed.proposals.isNotEmpty)
            _DriveSection(
              tok: tok,
              title: copy.organizerProposals,
              detail: copy.driveProposals(displayFeed.proposals.length),
              children: [
                for (final proposal in displayFeed.proposals)
                  _DriveProposalRow(
                    tok: tok,
                    copy: copy,
                    accent: widget.accent,
                    proposal: proposal,
                    onOpen:
                        proposal.sourceUri == null
                            ? null
                            : () => widget.onOpenResource(proposal.sourceUri!),
                  ),
              ],
            ),
          if (_loading) ...[
            const SizedBox(height: 10),
            LinearProgressIndicator(
              minHeight: 3,
              backgroundColor: tok.border.withValues(alpha: 0.5),
              valueColor: AlwaysStoppedAnimation<Color>(widget.accent),
            ),
          ],
          if (_error != null) ...[
            const SizedBox(height: 10),
            _DriveEmptyLine(
              tok: tok,
              icon: Icons.error_outline,
              text: copy.driveFeedLoadFailed(_error!),
            ),
          ],
        ],
      ],
    );
  }
}

class _DriveSection extends StatelessWidget {
  const _DriveSection({
    required this.tok,
    required this.title,
    required this.detail,
    required this.children,
  });

  final _Tok tok;
  final String title;
  final String detail;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 13),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  title,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12.5,
                    fontWeight: FontWeight.w900,
                  ),
                ),
              ),
              Text(
                detail,
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          ...children.expand((child) sync* {
            yield child;
            if (child != children.last) yield const SizedBox(height: 8);
          }),
        ],
      ),
    );
  }
}

class _DriveFeedDocRow extends StatelessWidget {
  const _DriveFeedDocRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.item,
    required this.onOpen,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final DriveFeedItem item;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: copy.openResource(item.uri),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-feed:${item.uri}'),
          onTap: onOpen,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: Container(
            padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
            decoration: BoxDecoration(
              color: tok.bg,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(12),
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Container(
                  width: 34,
                  height: 34,
                  decoration: BoxDecoration(
                    color: accent.withValues(alpha: 0.14),
                    border: Border.all(color: accent.withValues(alpha: 0.42)),
                    borderRadius: BorderRadius.circular(9),
                  ),
                  child: Icon(
                    Icons.insert_drive_file_outlined,
                    color: accent,
                    size: 17,
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        item.displayTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 13,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                      const SizedBox(height: 3),
                      Text(
                        item.displayPreview,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 11.5,
                          fontWeight: FontWeight.w600,
                          height: 1.35,
                        ),
                      ),
                      const SizedBox(height: 7),
                      Wrap(
                        spacing: 6,
                        runSpacing: 6,
                        children: [
                          if (item.docKind?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.description_outlined,
                              text: item.docKind!,
                            ),
                          if (item.project?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.folder_outlined,
                              text: item.project!,
                            ),
                          if (item.tags.isNotEmpty)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.label_outline,
                              text: copy.driveTags(item.tags.length),
                            ),
                          if (item.sizeBytes != null)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.data_object,
                              text: _formatDriveBytes(item.sizeBytes!),
                            ),
                          if (item.updatedAt?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.schedule,
                              text: _formatDriveUpdatedAt(
                                item.updatedAt!,
                                copy,
                              ),
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 6),
                Icon(Icons.chevron_right, color: tok.muted, size: 17),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _DriveVirtualDirChip extends StatelessWidget {
  const _DriveVirtualDirChip({
    required this.tok,
    required this.dir,
    required this.onOpen,
  });

  final _Tok tok;
  final DriveVirtualDir dir;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final label = dir.title.isEmpty ? dir.name : dir.title;
    return Semantics(
      button: true,
      label: label,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-dir:${dir.uri}'),
          onTap: onOpen,
          borderRadius: BorderRadius.circular(999),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 6),
            decoration: BoxDecoration(
              color: tok.bg,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(999),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(Icons.folder_special_outlined, color: tok.muted, size: 13),
                const SizedBox(width: 6),
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 190),
                  child: Text(
                    label,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 11.3,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _DriveProposalRow extends StatelessWidget {
  const _DriveProposalRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.proposal,
    required this.onOpen,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final DriveOrganizerProposal proposal;
  final VoidCallback? onOpen;

  @override
  Widget build(BuildContext context) {
    final confidence = proposal.confidence;
    final row = Container(
      padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              color: accent.withValues(alpha: 0.12),
              border: Border.all(color: accent.withValues(alpha: 0.38)),
              borderRadius: BorderRadius.circular(9),
            ),
            child: Icon(Icons.rule_folder_outlined, color: accent, size: 16),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        proposal.displayTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 12.7,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    Text(
                      proposal.status,
                      style: TextStyle(
                        color: accent,
                        fontSize: 10.5,
                        fontWeight: FontWeight.w900,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 3),
                Text(
                  proposal.displayPath,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11.3,
                    fontWeight: FontWeight.w600,
                    height: 1.35,
                  ),
                ),
                if (proposal.previewSnippet?.isNotEmpty == true) ...[
                  const SizedBox(height: 4),
                  Text(
                    proposal.previewSnippet!,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11,
                      fontWeight: FontWeight.w500,
                      height: 1.35,
                    ),
                  ),
                ],
                if (confidence != null) ...[
                  const SizedBox(height: 7),
                  _HistoryChip(
                    tok: tok,
                    icon: Icons.query_stats,
                    text: copy.driveConfidence(confidence),
                  ),
                ],
              ],
            ),
          ),
          if (onOpen != null) ...[
            const SizedBox(width: 6),
            Icon(Icons.chevron_right, color: tok.muted, size: 17),
          ],
        ],
      ),
    );
    if (onOpen == null) return row;
    return Semantics(
      button: true,
      label: proposal.sourceUri!,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onOpen,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: row,
        ),
      ),
    );
  }
}

class _DriveApprovalRow extends StatelessWidget {
  const _DriveApprovalRow({
    required this.tok,
    required this.copy,
    required this.approval,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final ApprovalPrompt approval;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: copy.pendingApprovalSemantics(approval.action),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-approval:${approval.approvalId}'),
          onTap: onTap,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: _DrivePendingShell(
            tok: tok,
            icon: Icons.warning_amber_rounded,
            title: approval.action,
            detail: copy.tapForDetails,
          ),
        ),
      ),
    );
  }
}

class _DrivePendingApprovalRow extends StatelessWidget {
  const _DrivePendingApprovalRow({required this.tok, required this.approval});

  final _Tok tok;
  final DrivePendingApproval approval;

  @override
  Widget build(BuildContext context) {
    return _DrivePendingShell(
      tok: tok,
      icon: Icons.pending_actions,
      title: approval.action,
      detail: approval.preview ?? approval.approvalId,
    );
  }
}

class _DrivePendingShell extends StatelessWidget {
  const _DrivePendingShell({
    required this.tok,
    required this.icon,
    required this.title,
    required this.detail,
  });

  final _Tok tok;
  final IconData icon;
  final String title;
  final String detail;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
      decoration: BoxDecoration(
        color: tok.warning.withValues(alpha: 0.1),
        border: Border.all(color: tok.warning.withValues(alpha: 0.48)),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          Icon(icon, color: tok.warning, size: 17),
          const SizedBox(width: 9),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12.5,
                    fontWeight: FontWeight.w800,
                  ),
                ),
                if (detail.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Text(
                    detail,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11.2,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _DriveEmptyLine extends StatelessWidget {
  const _DriveEmptyLine({
    required this.tok,
    required this.icon,
    required this.text,
  });

  final _Tok tok;
  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(12, 16, 12, 16),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(icon, color: tok.muted, size: 17),
          const SizedBox(width: 8),
          Flexible(
            child: Text(
              text,
              textAlign: TextAlign.center,
              style: TextStyle(
                color: tok.muted,
                fontSize: 12,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
