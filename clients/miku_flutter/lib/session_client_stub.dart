import 'dart:async';

import 'session_models.dart';

MikuSessionClient createClient() => ScriptedMikuClient();

class ScriptedMikuClient implements MikuSessionClient {
  final Map<String, StreamController<MikuEvent>> _controllers = {};
  final Map<String, MikuSession> _sessions = {};
  final Map<String, DateTime> _updatedAt = {};
  final Map<String, List<SessionMessage>> _messages = {};
  final Map<String, List<MikuEvent>> _pendingEvents = {};
  final Map<String, String> _approvalSessions = {};
  final Map<String, String> _approvalProposals = {};
  final Map<String, String> _approvalBackends = {};
  final Map<String, Map<String, Object?>> _proposals = {};
  final Map<String, String> rememberedLastEventIds = {};
  final List<String?> eventResumeIds = [];
  final List<String> resolvedApprovals = [];
  final List<String> lockedModes = [];
  final List<String> overriddenModes = [];
  int unlockCount = 0;
  int _nextId = 0;
  int _nextEventId = 1;
  String? _currentId;

  @override
  Future<ModeCatalog> modeCatalog() async {
    return const ModeCatalog(
      defaultMode: 'personal_assistant',
      modes: [
        ModeProfile(
          id: 'personal_assistant',
          label: 'Personal Assistant',
          voiceCap: 'medium',
          defaultScope: 'global',
          capabilityClass: 'conversation',
          activeSkills: ['miku-voice', 'personal-assistant-state-capture'],
          capabilities: ['memory.recall', 'memory.propose'],
          description: 'Planning, reminders, writing, and open loops.',
        ),
        ModeProfile(
          id: 'ambiguity_grill',
          label: 'Ambiguity Grill',
          voiceCap: 'high',
          defaultScope: 'global',
          capabilityClass: 'conversation',
          activeSkills: ['miku-voice', 'ambiguity-grill'],
          capabilities: [],
          description: 'Sharp clarification before planning.',
        ),
        ModeProfile(
          id: 'negative_state_grounding',
          label: 'Negative-State Grounding',
          voiceCap: 'high',
          defaultScope: 'global',
          capabilityClass: 'conversation',
          activeSkills: ['miku-voice', 'negative-state-grounding'],
          capabilities: [],
          description: 'Stabilize overwhelm before action.',
        ),
        ModeProfile(
          id: 'serious_engineer',
          label: 'Serious Engineer',
          voiceCap: 'off',
          defaultScope: 'project:tempestmiku',
          capabilityClass: 'engineering',
          activeSkills: [],
          capabilities: ['fs.*', 'code.*', 'proc.*', 'backend.coding'],
          description: 'Code, production, irreversible, or external work.',
        ),
        ModeProfile(
          id: 'handoff',
          label: 'Handoff',
          voiceCap: 'off',
          defaultScope: 'project:tempestmiku',
          capabilityClass: 'handoff',
          activeSkills: ['oh-my-pi-handoff'],
          capabilities: ['agents.*', 'backend.coding'],
          description: 'Delegate implementation-heavy coding work.',
        ),
      ],
    );
  }

  @override
  Future<MikuSession> createOrReuseSession() async {
    final currentId = _currentId;
    if (currentId != null && _sessions.containsKey(currentId)) {
      return _sessions[currentId]!;
    }
    return createSession();
  }

  @override
  Future<MikuSession> createSession() async {
    final id = 'scripted-${_nextId++}';
    final now = DateTime.now();
    final session = _sessionForMode(id, 'personal_assistant');
    _sessions[id] = session;
    _updatedAt[id] = now;
    _messages[id] = [];
    _pendingEvents[id] = [];
    _controllers[id] = StreamController<MikuEvent>.broadcast();
    _currentId = id;
    return session;
  }

  @override
  Future<List<SessionSummary>> listSessions({int limit = 30}) async {
    final ids = _sessions.keys.toList()
      ..sort((a, b) => (_updatedAt[b] ?? DateTime.fromMillisecondsSinceEpoch(0))
          .compareTo(_updatedAt[a] ?? DateTime.fromMillisecondsSinceEpoch(0)));
    return ids.take(limit).map((id) {
      final session = _sessions[id]!;
      final messages = _messages[id] ?? const [];
      final firstUser = messages
          .where((message) => message.role == 'user')
          .map((message) => message.content)
          .firstOrNull;
      final summary = messages.reversed
          .where((message) => message.role == 'assistant')
          .map((message) => message.content)
          .firstOrNull;
      final preview = messages.isEmpty ? '' : messages.last.content;
      return SessionSummary(
        id: id,
        title: _sessionTitle(summary, firstUser),
        preview: preview,
        mode: session.mode,
        label: session.label,
        updatedAt: (_updatedAt[id] ?? DateTime.now()).toIso8601String(),
        status: 'open',
        messageCount: messages.length,
        lastEventId: session.lastEventId,
      );
    }).toList();
  }

  @override
  Future<LoadedSession> loadSession(String sessionId) async {
    final base = _sessions[sessionId] ?? await createSession();
    final session = _copySession(
      base,
      lastEventId: rememberedLastEventIds[base.id] ?? base.lastEventId,
    );
    _currentId = session.id;
    return LoadedSession(
      session: session,
      messages: List<SessionMessage>.from(_messages[session.id] ?? const []),
      pendingEvents:
          List<MikuEvent>.from(_pendingEvents[session.id] ?? const []),
    );
  }

  @override
  Stream<MikuEvent> events(String sessionId, {String? lastEventId}) {
    eventResumeIds.add(lastEventId);
    return _controllers
        .putIfAbsent(sessionId, () => StreamController<MikuEvent>.broadcast())
        .stream;
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {
    rememberedLastEventIds[sessionId] = lastEventId;
  }

  @override
  Future<void> sendMessage(String sessionId, String content) async {
    final controller = _controllers[sessionId];
    if (controller == null) return;
    _appendMessage(sessionId, 'user', content);
    final lower = content.toLowerCase();
    if (lower.contains('actor') || lower.contains('handoff')) {
      _sessions[sessionId] = _sessionForMode(sessionId, 'handoff');
      controller.add(
        MikuEvent(
          type: 'mode',
          id: _eventId(),
          data: const {
            'mode': 'handoff',
            'label': 'Handoff',
            'voice_cap': 'off',
            'activeSkills': ['oh-my-pi-handoff'],
          },
        ),
      );
      controller.add(MikuEvent(
        type: 'actor_spawned',
        id: _eventId(),
        data: const {
          'actor_id': 'Worker0',
          'role': 'worker',
          'task': 'scripted actor smoke',
        },
      ));
      final approvalId = 'approval-${_nextEventId++}';
      _approvalSessions[approvalId] = sessionId;
      _approvalBackends[approvalId] = 'native-deno';
      final approvalEvent = MikuEvent(
        type: 'approval',
        id: _eventId(),
        data: {
          'approvalId': approvalId,
          'backend': 'native-deno',
          'action': 'proc.run cargo clean',
          'scope': const {
            'actorId': 'Worker0',
            'capability': 'proc.run',
          },
          'options': const [
            {
              'optionId': 'allow',
              'name': 'Allow once',
              'kind': 'allow_once',
            },
            {
              'optionId': 'reject',
              'name': 'Reject once',
              'kind': 'reject_once',
            },
          ],
          'timeoutMs': 60000,
        },
      );
      controller.add(approvalEvent);
      _pendingEvents.putIfAbsent(sessionId, () => []).add(approvalEvent);
      controller.add(MikuEvent(
        type: 'actor_completed',
        id: _eventId(),
        data: const {
          'actor_id': 'Worker0',
          'summary': 'scripted actor complete',
          'artifact_uri': 'artifact://0',
          'history_uri': 'history://Worker0',
        },
      ));
    } else if (lower.contains('code')) {
      _sessions[sessionId] = _sessionForMode(sessionId, 'serious_engineer');
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
    final text = lower.contains('actor') || lower.contains('handoff')
        ? 'Actor Worker0 completed child resource artifact://0'
        : 'Miku heard: $content';
    controller.add(MikuEvent(type: 'text', id: _eventId(), data: {
      'delta': text,
    }));
    controller.add(MikuEvent(type: 'final', id: _eventId(), data: {
      'text': text,
    }));
    _appendMessage(sessionId, 'assistant', text);
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
      );
      controller.add(approvalEvent);
      _pendingEvents.putIfAbsent(sessionId, () => []).add(approvalEvent);
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
    final proposalId = _approvalProposals[approvalId];
    _pendingEvents[sessionId]?.removeWhere((event) =>
        (event.type == 'approval' && event.data['approvalId'] == approvalId) ||
        (proposalId != null &&
            event.type == 'write_proposal' &&
            event.data['proposalId'] == proposalId));
    final approved = decision == 'approve';
    final backend = _approvalBackends[approvalId] ?? 'memory';
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
    lockedModes.add(mode);
    _sessions[sessionId] = _sessionForMode(sessionId, mode, locked: true);
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

  String _sessionTitle(String? summary, String? firstUser) {
    final summaryText = summary?.trim();
    if (summaryText != null && summaryText.isNotEmpty) return summaryText;
    final firstUserText = firstUser?.trim();
    if (firstUserText != null && firstUserText.isNotEmpty) {
      return firstUserText;
    }
    return 'New session';
  }

  @override
  Future<void> unlockMode(String sessionId) async {
    unlockCount++;
    _sessions[sessionId] = _sessionForMode(sessionId, 'personal_assistant');
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
  Future<void> overrideMode(String sessionId, String mode) async {
    overriddenModes.add(mode);
    _sessions[sessionId] = _sessionForMode(sessionId, mode);
    _controllers[sessionId]?.add(
      MikuEvent(
        type: 'mode',
        data: {
          'mode': mode,
          'label': _label(mode),
          'activeSkills': _activeSkills(mode),
          'override_source': 'user',
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

  MikuSession _sessionForMode(
    String id,
    String mode, {
    bool locked = false,
  }) {
    final lastEventId = _nextEventId > 1 ? '${_nextEventId - 1}' : null;
    return MikuSession(
      id: id,
      mode: mode,
      label: _label(mode),
      voiceCap: _voiceCap(mode),
      defaultScope: mode == 'serious_engineer' || mode == 'handoff'
          ? 'project:tempestmiku'
          : 'global',
      activeSkills: _activeSkills(mode),
      locked: locked,
      lastEventId: lastEventId,
    );
  }

  MikuSession _copySession(MikuSession session, {String? lastEventId}) {
    return MikuSession(
      id: session.id,
      mode: session.mode,
      label: session.label,
      voiceCap: session.voiceCap,
      defaultScope: session.defaultScope,
      activeSkills: session.activeSkills,
      locked: session.locked,
      lastEventId: lastEventId,
    );
  }

  String _voiceCap(String mode) {
    if (mode == 'serious_engineer' || mode == 'handoff') return 'off';
    if (mode == 'ambiguity_grill' || mode == 'negative_state_grounding') {
      return 'high';
    }
    return 'medium';
  }

  void _appendMessage(String sessionId, String role, String content) {
    final now = DateTime.now();
    final messages = _messages.putIfAbsent(sessionId, () => []);
    messages.add(
      SessionMessage(
        seq: messages.length + 1,
        role: role,
        content: content,
        createdAt: now.toIso8601String(),
      ),
    );
    _updatedAt[sessionId] = now;
    final session = _sessions[sessionId];
    if (session != null) {
      _sessions[sessionId] = MikuSession(
        id: session.id,
        mode: session.mode,
        label: session.label,
        voiceCap: session.voiceCap,
        defaultScope: session.defaultScope,
        activeSkills: session.activeSkills,
        locked: session.locked,
        lastEventId: '${_nextEventId - 1}',
      );
    }
  }

  String _eventId() => '${_nextEventId++}';
}
