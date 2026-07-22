part of '../session_client_stub.dart';

extension _ScriptedProposalScenarios on ScriptedMikuClient {
  Future<MemoryWriteProposalResult> _proposeMemoryWriteImpl(
    String sessionId,
    MemoryWriteProposalRequest request,
  ) {
    _requireActiveProposalSession(sessionId);
    final preview = switch (request.memoryKind) {
      'profile_fact' when request.predicate != null && request.object != null =>
        'brian ${request.predicate} ${request.object}',
      'recall_chunk' when request.text != null => request.text!,
      'profile_fact' =>
        throw StateError('memory predicate and object are required'),
      'recall_chunk' => throw StateError('memory text is required'),
      _ => throw StateError('unknown memory kind ${request.memoryKind}'),
    };
    final proposalId = 'proposal-${_nextEventId++}';
    final approvalId = 'approval-${_nextEventId++}';
    final recordId = 'record-${_nextEventId++}';
    final createdAt = DateTime.now().toUtc().toIso8601String();
    final proposal = <String, Object?>{
      'kind': 'memory',
      'proposalId': proposalId,
      'memoryKind': request.memoryKind,
      'status': 'pending',
      'preview': preview,
      'uri': 'memory://evolution-proposals/$proposalId',
      'contentDigest': 'scripted:$proposalId',
      'recordId': recordId,
      'createdAt': createdAt,
    };
    const options = [
      {'optionId': 'allow', 'name': 'Save memory', 'kind': 'allow_once'},
      {'optionId': 'reject', 'name': 'Reject memory', 'kind': 'reject_once'},
    ];
    final approvalScope = <String, Object?>{
      'proposal': {
        'kind': 'memory',
        'proposalId': proposalId,
        'memoryKind': request.memoryKind,
        'preview': preview,
        'uri': 'memory://evolution-proposals/$proposalId',
        'contentDigest': 'scripted:$proposalId',
        'recordId': recordId,
      },
      'timeoutMs': request.timeoutMs ?? 60000,
    };
    _registerScriptedProposal(
      sessionId,
      approvalId: approvalId,
      proposalId: proposalId,
      backend: 'memory',
      action: 'memory.write ${request.memoryKind}: $preview',
      scope: approvalScope,
      options: options,
      proposal: proposal,
    );
    final completer = Completer<MemoryWriteProposalResult>();
    _memoryProposalCompleters[approvalId] = completer;
    return completer.future;
  }

  Future<EvolutionReviewProposalResult> _proposeEvolutionReviewImpl(
    String sessionId,
    EvolutionReviewProposalRequest request,
  ) async {
    _requireActiveProposalSession(sessionId);
    if (request.changes.isEmpty) {
      throw StateError('evolution review changes are required');
    }
    final proposalId = 'review-${_nextEventId++}';
    final approvalId = 'approval-${_nextEventId++}';
    final preview = request.changes
        .map((change) => '${change.after.label}: ${change.after.summary}')
        .join('\n');
    final resourceUri = 'memory://review-proposals/$proposalId';
    final applyEnabled =
        request.target.kind == 'mode' ||
        request.changes.every(
          (change) => const {
            'tone_guidance',
            'address_guidance',
            'interaction_preference',
          }.contains(change.section),
        );
    final target = request.target.toJson();
    final createdAt = DateTime.now().toUtc().toIso8601String();
    final proposal = <String, Object?>{
      'kind': 'evolution_review',
      'proposalId': proposalId,
      'target': target,
      'status': 'pending',
      'baseVersion': 1,
      'baseDigest': 'scripted-base:${request.target.id}',
      'preview': preview,
      'contentDigest': 'scripted:$proposalId',
      'uri': resourceUri,
      'applyEnabled': applyEnabled,
      'createdAt': createdAt,
      'updatedAt': createdAt,
    };
    const options = [
      {
        'optionId': 'allow',
        'name': 'Apply reviewed addendum',
        'kind': 'allow_once',
      },
      {'optionId': 'reject', 'name': 'Reject proposal', 'kind': 'reject_once'},
    ];
    final approvalScope = <String, Object?>{
      'kind': 'evolution_review',
      'proposalId': proposalId,
      'target': target,
      'baseVersion': 1,
      'baseDigest': 'scripted-base:${request.target.id}',
      'preview': preview,
      'contentDigest': 'scripted:$proposalId',
      'uri': resourceUri,
      'applyEnabled': applyEnabled,
      'timeoutMs': request.timeoutMs ?? 60000,
    };
    _registerScriptedProposal(
      sessionId,
      approvalId: approvalId,
      proposalId: proposalId,
      backend: 'evolution-review',
      action: 'review ${request.target.kind} addendum ${request.target.id}',
      scope: approvalScope,
      options: options,
      proposal: proposal,
    );
    return EvolutionReviewProposalResult(
      proposalId: proposalId,
      approvalId: approvalId,
      status: 'pending',
      resourceUri: resourceUri,
      applyEnabled: applyEnabled,
    );
  }

  Future<ModeAddendumRollbackResult> _proposeModeAddendumRollbackImpl(
    String sessionId,
    String modeId,
    AddendumRollbackRequest request,
  ) async {
    final approvalId = _registerScriptedRollback(
      sessionId,
      kind: 'mode_addendum_rollback',
      targetField: 'modeId',
      targetId: modeId,
      expectedActiveDigest: request.expectedActiveDigest,
      targetDigest: request.targetDigest,
      timeoutMs: request.timeoutMs,
      backend: 'mode-addendum-rollback',
      action: 'mode.addendum.rollback $modeId',
    );
    return ModeAddendumRollbackResult(
      approvalId: approvalId,
      modeId: modeId,
      expectedActiveDigest: request.expectedActiveDigest,
      targetDigest: request.targetDigest,
      status: 'pending',
    );
  }

  Future<PersonaAddendumRollbackResult> _proposePersonaAddendumRollbackImpl(
    String sessionId,
    String personaId,
    AddendumRollbackRequest request,
  ) async {
    final approvalId = _registerScriptedRollback(
      sessionId,
      kind: 'persona_addendum_rollback',
      targetField: 'personaId',
      targetId: personaId,
      expectedActiveDigest: request.expectedActiveDigest,
      targetDigest: request.targetDigest,
      timeoutMs: request.timeoutMs,
      backend: 'persona-addendum-rollback',
      action: 'persona.addendum.rollback $personaId',
    );
    return PersonaAddendumRollbackResult(
      approvalId: approvalId,
      personaId: personaId,
      expectedActiveDigest: request.expectedActiveDigest,
      targetDigest: request.targetDigest,
      status: 'pending',
    );
  }

  Future<SkillRollbackResult> _proposeSkillRollbackImpl(
    String sessionId,
    String skillName,
    SkillRollbackRequest request,
  ) async {
    final approvalId = _registerScriptedRollback(
      sessionId,
      kind: 'skill_rollback',
      targetField: 'name',
      targetId: skillName,
      expectedActiveDigest: request.expectedActiveDigest,
      targetDigest: request.targetDigest,
      timeoutMs: request.timeoutMs,
      backend: 'skill-rollback',
      action: 'skill.rollback $skillName',
    );
    return SkillRollbackResult(
      approvalId: approvalId,
      name: skillName,
      expectedActiveDigest: request.expectedActiveDigest,
      targetDigest: request.targetDigest,
      status: 'pending',
    );
  }

  Future<ApprovalDetails> _getApprovalImpl(
    String sessionId,
    String approvalId,
  ) async {
    if (_approvalSessions[approvalId] != sessionId) {
      throw StateError('unknown approval $approvalId for session $sessionId');
    }
    final detail = _approvalDetails[approvalId];
    if (detail == null) {
      throw StateError('approval $approvalId has no scripted detail');
    }
    return ApprovalDetails.fromJson({
      ...detail,
      'serverTime': DateTime.now().toUtc().toIso8601String(),
    });
  }

  MikuSession _requireActiveProposalSession(String sessionId) {
    final session = _sessions[sessionId];
    if (session == null) throw StateError('unknown session $sessionId');
    if (session.status == 'ended') {
      throw StateError('session $sessionId has ended');
    }
    return session;
  }

  String _registerScriptedRollback(
    String sessionId, {
    required String kind,
    required String targetField,
    required String targetId,
    required String expectedActiveDigest,
    required String? targetDigest,
    required int? timeoutMs,
    required String backend,
    required String action,
  }) {
    _requireActiveProposalSession(sessionId);
    final approvalId = 'approval-${_nextEventId++}';
    final proposalId = 'rollback-${_nextEventId++}';
    final proposal = <String, Object?>{
      'kind': kind,
      'status': 'pending',
      targetField: targetId,
      'expectedActiveDigest': expectedActiveDigest,
      'targetDigest': targetDigest,
      'targetProposalId': proposalId,
    };
    const options = [
      {'optionId': 'allow', 'name': 'Roll back', 'kind': 'allow_once'},
      {
        'optionId': 'reject',
        'name': 'Keep current version',
        'kind': 'reject_once',
      },
    ];
    _registerScriptedProposal(
      sessionId,
      approvalId: approvalId,
      proposalId: proposalId,
      backend: backend,
      action: action,
      scope: {...proposal, 'timeoutMs': timeoutMs ?? 60000}..remove('status'),
      options: options,
      proposal: proposal,
    );
    return approvalId;
  }

  void _registerScriptedProposal(
    String sessionId, {
    required String approvalId,
    required String proposalId,
    required String backend,
    required String action,
    required Map<String, Object?> scope,
    required List<Map<String, Object?>> options,
    required Map<String, Object?> proposal,
  }) {
    _approvalSessions[approvalId] = sessionId;
    _approvalProposals[approvalId] = proposalId;
    _approvalBackends[approvalId] = backend;
    _proposals[proposalId] = proposal;
    _recordPendingApproval(
      sessionId,
      approvalId: approvalId,
      backend: backend,
      action: action,
      scope: scope,
      options: options,
    );
    final proposalEvent = MikuEvent(
      type: 'write_proposal',
      id: _eventId(),
      data: proposal,
    );
    _approvalProposalEventIds[approvalId] = proposalEvent.id!;
    final approvalEvent = MikuEvent(
      type: 'approval',
      id: _eventId(),
      data: {
        'approvalId': approvalId,
        'backend': backend,
        'action': action,
        'scope': scope,
        'options': options,
        'timeoutMs': scope['timeoutMs'] ?? 60000,
      },
    );
    _pendingEvents.putIfAbsent(sessionId, () => []).addAll([
      proposalEvent,
      approvalEvent,
    ]);
    final controller = _controllers[sessionId];
    controller?.add(proposalEvent);
    controller?.add(approvalEvent);
  }

  void _recordPendingApproval(
    String sessionId, {
    required String approvalId,
    required String backend,
    required String action,
    required Map<String, Object?> scope,
    required List<Map<String, Object?>> options,
  }) {
    final now = DateTime.now().toUtc();
    final timeoutMs = (scope['timeoutMs'] as num?)?.toInt() ?? 60000;
    _approvalDetails[approvalId] = {
      'approvalId': approvalId,
      'sessionId': sessionId,
      'backend': backend,
      'action': action,
      'scope': scope,
      'options': options,
      'status': 'pending',
      'createdAt': now.toIso8601String(),
      'expiresAt': now.add(Duration(milliseconds: timeoutMs)).toIso8601String(),
      'resolvedAt': null,
      'serverTime': now.toIso8601String(),
    };
  }

  void _completeScriptedMemoryProposal(
    String approvalId, {
    required bool approved,
  }) {
    final completer = _memoryProposalCompleters.remove(approvalId);
    if (completer == null || completer.isCompleted) return;
    final proposalId = _approvalProposals[approvalId];
    final proposal = proposalId == null ? null : _proposals[proposalId];
    if (proposal == null) {
      completer.completeError(StateError('missing scripted memory proposal'));
      return;
    }
    final memoryKind = proposal['memoryKind']?.toString() ?? '';
    final recordId = proposal['recordId']?.toString() ?? '';
    final sessionId = _approvalSessions[approvalId];
    final session = _sessions[sessionId];
    final scope =
        session?.memoryPolicy == MikuMemoryPolicy.project &&
                session?.projectId != null
            ? 'project:${session!.projectId}'
            : 'global';
    final uri =
        memoryKind == 'profile_fact'
            ? 'memory://profile/brian/facts/$recordId'
            : 'memory://scopes/${Uri.encodeComponent(scope)}/chunks/$recordId';
    completer.complete(
      MemoryWriteProposalResult(
        proposalId: proposalId!,
        memoryKind: memoryKind,
        status: approved ? 'approved' : 'denied',
        record:
            approved
                ? MemoryRecordReference(
                  id: recordId,
                  uri: uri,
                  kind: memoryKind,
                )
                : null,
      ),
    );
  }
}
