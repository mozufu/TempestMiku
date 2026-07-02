import 'dart:async';

import 'session_models.dart';

MikuSessionClient createClient() => ScriptedMikuClient();

class ScriptedMikuClient implements MikuSessionClient {
  final Map<String, StreamController<MikuEvent>> _controllers = {};
  final Map<String, String> _approvalSessions = {};
  final Map<String, String> _approvalProposals = {};
  final Map<String, Map<String, Object?>> _proposals = {};
  final List<String> resolvedApprovals = [];
  int _nextId = 0;
  int _nextEventId = 1;

  @override
  Future<MikuSession> createOrReuseSession() => createSession();

  @override
  Future<MikuSession> createSession() async {
    final id = 'scripted-${_nextId++}';
    _controllers[id] = StreamController<MikuEvent>.broadcast();
    return MikuSession(
      id: id,
      mode: 'personal_assistant',
      label: 'Personal Assistant',
      voiceCap: 'medium',
      activeSkills: const ['miku-voice', 'personal-assistant-state-capture'],
    );
  }

  @override
  Stream<MikuEvent> events(String sessionId, {String? lastEventId}) {
    return _controllers
        .putIfAbsent(sessionId, () => StreamController<MikuEvent>.broadcast())
        .stream;
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {}

  @override
  Future<void> sendMessage(String sessionId, String content) async {
    final controller = _controllers[sessionId];
    if (controller == null) return;
    if (content.toLowerCase().contains('code')) {
      controller.add(
        MikuEvent(
          type: 'mode',
          id: _eventId(),
          data: const {
            'mode': 'serious_engineer',
            'label': 'Serious Engineer',
            'voice_cap': 'off',
            'activeSkills': [],
          },
        ),
      );
    }
    final text = 'Miku heard: $content';
    controller.add(MikuEvent(type: 'text', id: _eventId(), data: {
      'delta': text,
    }));
    controller.add(MikuEvent(type: 'final', id: _eventId(), data: {
      'text': text,
    }));
    if (content.toLowerCase().contains('remember')) {
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
        'source': 'scripted-widget-test',
        'provenanceLabel': 'scripted chat turn',
        'provenance': {'sessionId': sessionId},
        'dedupeKey': 'scripted-memory-proposal',
        'recordId': 'record-scripted',
      };
      _approvalSessions[approvalId] = sessionId;
      _approvalProposals[approvalId] = proposalId;
      _proposals[proposalId] = proposal;
      controller.add(
        MikuEvent(
          type: 'write_proposal',
          id: _eventId(),
          data: proposal,
        ),
      );
      controller.add(
        MikuEvent(
          type: 'approval',
          id: _eventId(),
          data: {
            'approvalId': approvalId,
            'backend': 'memory',
            'action': 'memory.write profile_fact',
            'scope': {
              'proposal': proposal,
              'timeoutMs': 60000,
            },
            'options': const [
              {
                'optionId': 'allow',
                'name': 'Save memory',
                'kind': 'allow_once',
              },
              {
                'optionId': 'reject',
                'name': 'Reject memory',
                'kind': 'reject_once',
              },
            ],
            'timeoutMs': 60000,
          },
        ),
      );
    }
  }

  @override
  Future<void> resolveApproval(
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
    final approved = decision == 'approve';
    controller.add(
      MikuEvent(
        type: 'approval_resolved',
        id: _eventId(),
        data: {
          'approvalId': approvalId,
          'backend': 'memory',
          'status': approved ? 'approved' : 'denied',
          'outcome': 'selected',
          'optionId': approved ? 'allow' : 'reject',
        },
      ),
    );
    final proposalId = _approvalProposals[approvalId];
    final proposal = _proposals[proposalId];
    if (proposal == null) return;
    controller.add(
      MikuEvent(
        type: 'write_proposal',
        id: _eventId(),
        data: {
          ...proposal,
          'status': approved ? 'approved' : 'denied',
          if (approved)
            'record': {
              'id': proposal['recordId'],
              'uri': 'memory://profile/brian/facts/${proposal['recordId']}',
              'kind': proposal['memoryKind'],
            },
        },
      ),
    );
  }

  @override
  Future<void> lockMode(String sessionId, String mode) async {
    _controllers[sessionId]?.add(
      MikuEvent(
        type: 'mode',
        data: {
          'mode': mode,
          'label': _label(mode),
          'activeSkills': _activeSkills(mode),
          'lock_source': 'user',
        },
      ),
    );
  }

  @override
  Future<void> unlockMode(String sessionId) async {
    _controllers[sessionId]?.add(
      const MikuEvent(
        type: 'mode',
        data: {
          'mode': 'personal_assistant',
          'label': 'Personal Assistant',
          'activeSkills': ['miku-voice', 'personal-assistant-state-capture'],
        },
      ),
    );
  }

  @override
  Future<ProjectOverview> projectOverview(String sessionId) async {
    return const ProjectOverview(
      status: 'Scripted project status',
      nextActions: ['Continue from latest session result'],
    );
  }

  @override
  Future<ResourcePreview> previewResource(String sessionId, String uri) async {
    return ResourcePreview(
      uri: uri,
      kind: 'text',
      mime: 'text/plain',
      title: 'Scripted resource',
      sizeBytes: 48,
      preview: 'Preview for $uri',
      hasMore: false,
    );
  }

  @override
  Future<ProjectPromotion> promoteSession(
    String sessionId, {
    String? summary,
    List<String> openLoops = const [],
    List<String> decisions = const [],
    List<String> resources = const [],
  }) async {
    return ProjectPromotion(
      projectUri: 'project://tempestmiku',
      promotedCount: resources.length + (summary == null ? 0 : 1),
    );
  }

  String _label(String mode) {
    return switch (mode) {
      'serious_engineer' => 'Serious Engineer',
      'handoff' => 'Handoff',
      'ambiguity_grill' => 'Ambiguity Grill',
      'negative_state_grounding' => 'Negative-State Grounding',
      _ => 'Personal Assistant',
    };
  }

  List<String> _activeSkills(String mode) {
    return switch (mode) {
      'ambiguity_grill' => const ['miku-voice', 'ambiguity-grill'],
      'negative_state_grounding' => const [
          'miku-voice',
          'negative-state-grounding',
        ],
      'serious_engineer' => const [],
      'handoff' => const ['oh-my-pi-handoff'],
      _ => const ['miku-voice', 'personal-assistant-state-capture'],
    };
  }

  String _eventId() => '${_nextEventId++}';
}
