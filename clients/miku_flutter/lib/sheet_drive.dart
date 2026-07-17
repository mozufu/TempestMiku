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
      padding: const EdgeInsets.fromLTRB(15, 9, 15, 18),
      children: [
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
            const SizedBox(width: 8),
            _TokIconBtn(
              tok: tok,
              icon: Icons.close,
              tooltip: copy.close,
              semanticLabel: copy.closeDriveFeed,
              onTap: () => Navigator.pop(context),
            ),
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
