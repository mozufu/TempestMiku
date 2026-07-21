part of '../session_client_stub.dart';

extension _ScriptedMessageScenarios on ScriptedMikuClient {
  Future<void> _sendScriptedMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) async {
    sentClientMessageIds.add(clientMessageId);
    final controller = _controllers[sessionId];
    if (controller == null) return;
    if (!_acceptedMessageKeys.add('$sessionId\u{0}$clientMessageId')) return;
    _appendMessage(sessionId, 'user', content);
    final lower = content.toLowerCase();
    if (lower.contains('actor') || lower.contains('handoff')) {
      _sessions[sessionId] = _sessionForMode(sessionId, 'serious_engineer');
      controller.add(
        MikuEvent(
          type: 'mode',
          id: _eventId(),
          data: const {
            'mode': 'serious_engineer',
            'label': 'Serious Engineer',
            'activeSkills': <String>[],
          },
        ),
      );
      controller.add(
        MikuEvent(
          type: 'tool_call',
          id: _eventId(),
          data: const {'name': 'execute'},
        ),
      );
      controller.add(
        MikuEvent(
          type: 'cell_start',
          id: _eventId(),
          data: const {
            'sourcePreview':
                'const worker = await agents.spawn("worker", "scripted actor smoke");\n'
                'display(await agents.wait(worker, 5000));',
          },
        ),
      );
      controller.add(
        MikuEvent(
          type: 'actor_spawned',
          id: _eventId(),
          data: const {
            'actor_id': 'Worker0',
            'role': 'worker',
            'task': 'scripted actor smoke',
          },
        ),
      );
      final approvalId = 'approval-${_nextEventId++}';
      _approvalSessions[approvalId] = sessionId;
      _approvalBackends[approvalId] = 'native-tm';
      const approvalScope = {
        'actorId': 'Worker0',
        'capability': 'proc.run',
        'timeoutMs': 60000,
      };
      const approvalOptions = [
        {'optionId': 'allow', 'name': 'Allow once', 'kind': 'allow_once'},
        {'optionId': 'reject', 'name': 'Reject once', 'kind': 'reject_once'},
      ];
      _recordPendingApproval(
        sessionId,
        approvalId: approvalId,
        backend: 'native-tm',
        action: 'proc.run cargo clean',
        scope: approvalScope,
        options: approvalOptions,
      );
      final approvalEvent = MikuEvent(
        type: 'approval',
        id: _eventId(),
        data: {
          'approvalId': approvalId,
          'backend': 'native-tm',
          'action': 'proc.run cargo clean',
          'scope': approvalScope,
          'options': approvalOptions,
          'timeoutMs': 60000,
        },
      );
      controller.add(approvalEvent);
      _pendingEvents.putIfAbsent(sessionId, () => []).add(approvalEvent);
      controller.add(
        MikuEvent(
          type: 'actor_completed',
          id: _eventId(),
          data: const {
            'actor_id': 'Worker0',
            'summary': 'scripted actor complete',
            'artifact_uri': 'artifact://0',
            'history_uri': 'history://Worker0',
          },
        ),
      );
      controller.add(
        MikuEvent(
          type: 'cell_result',
          id: _eventId(),
          data: const {
            'status': 'completed',
            'resultPreview': 'scripted actor complete',
          },
        ),
      );
    } else if (lower.contains('code')) {
      _sessions[sessionId] = _sessionForMode(sessionId, 'serious_engineer');
      controller.add(
        MikuEvent(
          type: 'mode',
          id: _eventId(),
          data: const {
            'mode': 'serious_engineer',
            'label': 'Serious Engineer',
            'activeSkills': [],
          },
        ),
      );
    }
    if (lower.contains('reasoning')) {
      controller.add(
        MikuEvent(
          type: 'reasoning',
          id: _eventId(),
          data: const {
            'delta':
                'Compare scheduler invariants, approval gates, and replay traces before answering.',
          },
        ),
      );
    }
    final wantsDriveWorkspace =
        lower.contains('drive') || lower.contains('research');
    if (wantsDriveWorkspace) {
      _emitDriveWorkspace(sessionId, controller);
    }
    final text =
        wantsDriveWorkspace
            ? 'Drive research workspace ready: '
                'drive://projects/tempestmiku/research/p5-drive-workspace.md'
            : lower.contains('markdown')
            ? '# P4 memo\n\n'
                '> Proposal-first background work.\n\n'
                '- **Keep approvals manual** for durable writes.\n'
                '- [ ] Rebuild projections from the event log.\n\n'
                r'\\[ \\sin z = \\frac{e^{iz}-e^{-iz}}{2i} \\]'
                '\n\n'
                r'Inline math \(e^{i\pi}+1=0\).'
                '\n\n'
                'Use `write_proposal` before memory commit.'
            : lower.contains('actor') || lower.contains('handoff')
            ? 'Actor Worker0 completed child resource artifact://0'
            : 'Miku heard: $content';
    controller.add(
      MikuEvent(type: 'text', id: _eventId(), data: {'delta': text}),
    );
    if (pauseBeforeFinal) {
      _pausedFinalTexts[sessionId] = text;
      return;
    }
    _emitFinal(sessionId, text);
    if (lower.contains('remember') || lower.contains('dream')) {
      final dreamOrigin = lower.contains('dream');
      final proposalId = 'proposal-${_nextEventId++}';
      final approvalId = 'approval-${_nextEventId++}';
      final proposal = <String, Object?>{
        'kind': 'memory',
        'proposalId': proposalId,
        'memoryKind': 'profile_fact',
        'status': 'pending',
        'subject': 'brian',
        'scope': 'global',
        'text': 'Brian prefers approval-backed memory writes.',
        'predicate': 'prefers',
        'object': 'approval-backed memory writes',
        'confidence': 0.82,
        'source':
            dreamOrigin ? 'dream:scripted-widget-test' : 'scripted-widget-test',
        'provenanceLabel':
            dreamOrigin ? 'post-session-dream' : 'scripted chat turn',
        'provenance': {
          'sessionId': sessionId,
          if (dreamOrigin) 'sourceDream': 'dream-scripted',
        },
        'dedupeKey': 'scripted-memory-proposal',
        'recordId': 'record-scripted',
      };
      _approvalSessions[approvalId] = sessionId;
      _approvalProposals[approvalId] = proposalId;
      _approvalBackends[approvalId] = 'memory';
      _proposals[proposalId] = proposal;
      final proposalEvent = MikuEvent(
        type: 'write_proposal',
        id: _eventId(),
        data: proposal,
      );
      controller.add(proposalEvent);
      _pendingEvents.putIfAbsent(sessionId, () => []).add(proposalEvent);
      final approvalEvent = MikuEvent(
        type: 'approval',
        id: _eventId(),
        data: {
          'approvalId': approvalId,
          'backend': 'memory',
          'action': 'memory.write profile_fact',
          'scope': {'proposal': proposal, 'timeoutMs': 60000},
          'options': const [
            {'optionId': 'allow', 'name': 'Save memory', 'kind': 'allow_once'},
            {
              'optionId': 'reject',
              'name': 'Reject memory',
              'kind': 'reject_once',
            },
          ],
          'timeoutMs': 60000,
        },
      );
      _recordPendingApproval(
        sessionId,
        approvalId: approvalId,
        backend: 'memory',
        action: 'memory.write profile_fact',
        scope: (approvalEvent.data['scope'] as Map).cast<String, Object?>(),
        options:
            (approvalEvent.data['options'] as List)
                .whereType<Map>()
                .map((item) => item.cast<String, Object?>())
                .toList(),
      );
      controller.add(approvalEvent);
      _pendingEvents.putIfAbsent(sessionId, () => []).add(approvalEvent);
    }
  }

  Future<void> _resolveScriptedApproval(
    String sessionId,
    String approvalId,
    String decision, {
    String? optionId,
  }) async {
    resolvedApprovals.add('$approvalId:$decision');
    final controller = _controllers[sessionId];
    if (controller == null || _approvalSessions[approvalId] != sessionId) {
      return;
    }
    final proposalId = _approvalProposals[approvalId];
    final proposalEventId = _approvalProposalEventIds[approvalId];
    _pendingEvents[sessionId]?.removeWhere(
      (event) =>
          (event.type == 'approval' &&
              event.data['approvalId'] == approvalId) ||
          (proposalId != null &&
              event.type == 'write_proposal' &&
              (event.data['proposalId'] == proposalId ||
                  event.id == proposalEventId)),
    );
    final approved = decision == 'approve';
    final backend = _approvalBackends[approvalId] ?? 'memory';
    final detail = _approvalDetails[approvalId];
    if (detail != null) {
      final resolvedAt = DateTime.now().toUtc().toIso8601String();
      _approvalDetails[approvalId] = {
        ...detail,
        'status': approved ? 'approved' : 'denied',
        'resolvedAt': resolvedAt,
        'serverTime': resolvedAt,
      };
    }
    controller.add(
      MikuEvent(
        type: 'approval_resolved',
        id: _eventId(),
        data: {
          'approvalId': approvalId,
          'backend': backend,
          'status': approved ? 'approved' : 'denied',
          'outcome': 'selected',
          'optionId': approved ? 'allow' : 'reject',
        },
      ),
    );
    final proposal = _proposals[proposalId];
    if (proposal == null) {
      _completeScriptedMemoryProposal(approvalId, approved: approved);
      return;
    }
    final isMemory = proposal['kind'] == 'memory';
    controller.add(
      MikuEvent(
        type: 'write_proposal',
        id: _eventId(),
        data: {
          ...proposal,
          'status': approved ? 'approved' : 'denied',
          if (approved && isMemory)
            'record': {
              'id': proposal['recordId'],
              'uri': 'memory://profile/brian/facts/${proposal['recordId']}',
              'kind': proposal['memoryKind'],
            },
        },
      ),
    );
    _completeScriptedMemoryProposal(approvalId, approved: approved);
  }
}
