part of 'conversation_screen.dart';

sealed class _ReviewedChangeRequest {
  const _ReviewedChangeRequest();

  String get pendingLabel;
}

class _ReviewedMemoryRequest extends _ReviewedChangeRequest {
  const _ReviewedMemoryRequest(this.request);

  final MemoryWriteProposalRequest request;

  @override
  String get pendingLabel => '記憶變更已提出，請在對話中的確認卡片審核。';
}

class _ReviewedEvolutionRequest extends _ReviewedChangeRequest {
  const _ReviewedEvolutionRequest(this.request);

  final EvolutionReviewProposalRequest request;

  @override
  String get pendingLabel => 'Guidance 變更已提出，核准前不會啟用。';
}

enum _RollbackTargetKind { mode, persona, skill }

class _ReviewedRollbackRequest extends _ReviewedChangeRequest {
  const _ReviewedRollbackRequest({
    required this.kind,
    required this.targetId,
    required this.expectedActiveDigest,
    required this.targetDigest,
  });

  final _RollbackTargetKind kind;
  final String targetId;
  final String expectedActiveDigest;
  final String? targetDigest;

  @override
  String get pendingLabel => 'Rollback 已提出，核准前不會切換版本。';
}

extension _ConversationReviewedChanges on _ConversationScreenState {
  Future<void> _openReviewedChanges() async {
    final session = _session;
    if (session == null || session.status == 'ended') return;
    if (_reviewedChangeInFlight) {
      _voiceSetState(() {
        _items.add(
          _NoticeItem(
            key: _nextKey('reviewed-change-busy'),
            text: '上一個變更提案仍在處理中，請稍候再試一次。',
          ),
        );
      });
      _scheduleScroll(force: true);
      return;
    }
    final request = await showModalBottomSheet<_ReviewedChangeRequest>(
      context: context,
      useSafeArea: true,
      isScrollControlled: true,
      showDragHandle: true,
      builder:
          (context) => _ReviewedChangesSheet(
            catalog: _modeCatalog,
            currentModeId: session.mode,
          ),
    );
    if (request == null || !mounted || _session?.id != session.id) return;
    unawaited(_executeReviewedChange(session.id, request));
  }

  Future<void> _executeReviewedChange(
    String sessionId,
    _ReviewedChangeRequest request,
  ) async {
    if (!mounted || _session?.id != sessionId) return;
    _reviewedChangeInFlight = true;
    _voiceSetState(() {
      _items.add(
        _NoticeItem(
          key: _nextKey('reviewed-change'),
          text: request.pendingLabel,
        ),
      );
    });
    _scheduleScroll(force: true);
    try {
      switch (request) {
        case _ReviewedMemoryRequest(:final request):
          // This endpoint deliberately remains open until the durable manual
          // approval resolves. The SSE approval card is the interactive surface.
          await widget.client.proposeMemoryWrite(sessionId, request);
        case _ReviewedEvolutionRequest(:final request):
          final result = await widget.client.proposeEvolutionReview(
            sessionId,
            request,
          );
          await _surfaceReviewedApproval(sessionId, result.approvalId);
        case _ReviewedRollbackRequest(
          :final kind,
          :final targetId,
          :final expectedActiveDigest,
          :final targetDigest,
        ):
          final approvalId = switch (kind) {
            _RollbackTargetKind.mode =>
              (await widget.client.proposeModeAddendumRollback(
                sessionId,
                targetId,
                AddendumRollbackRequest(
                  expectedActiveDigest: expectedActiveDigest,
                  targetDigest: targetDigest,
                ),
              )).approvalId,
            _RollbackTargetKind.persona =>
              (await widget.client.proposePersonaAddendumRollback(
                sessionId,
                targetId,
                AddendumRollbackRequest(
                  expectedActiveDigest: expectedActiveDigest,
                  targetDigest: targetDigest,
                ),
              )).approvalId,
            _RollbackTargetKind.skill =>
              (await widget.client.proposeSkillRollback(
                sessionId,
                targetId,
                SkillRollbackRequest(
                  expectedActiveDigest: expectedActiveDigest,
                  targetDigest: targetDigest!,
                ),
              )).approvalId,
          };
          await _surfaceReviewedApproval(sessionId, approvalId);
      }
    } catch (_) {
      if (!mounted || _session?.id != sessionId) return;
      _voiceSetState(() {
        _items.add(
          _NoticeItem(
            key: _nextKey('reviewed-change-error'),
            text: '變更提案沒有建立。伺服器可能拒絕了內容、目標或過期的版本 digest。',
            isError: true,
          ),
        );
      });
      _scheduleScroll(force: true);
    } finally {
      _reviewedChangeInFlight = false;
    }
  }

  Future<void> _surfaceReviewedApproval(
    String sessionId,
    String approvalId,
  ) async {
    try {
      final details = await widget.client.getApproval(sessionId, approvalId);
      if (!mounted || _session?.id != sessionId) return;
      _voiceSetState(() {
        if (details.isPending &&
            !_items.whereType<_ApprovalItem>().any(
              (item) => item.prompt.approvalId == approvalId,
            )) {
          _items.add(
            _ApprovalItem(key: 'approval-$approvalId', prompt: details.prompt),
          );
        }
      });
      _scheduleScroll(force: true);
    } catch (_) {
      // The same durable approval is also delivered on SSE. A transient GET
      // failure must not duplicate or invalidate that source of truth.
    }
  }
}
