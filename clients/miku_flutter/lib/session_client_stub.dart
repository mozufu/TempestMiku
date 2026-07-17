import 'dart:async';

import 'session_models.dart';

MikuSessionClient createClient() => ScriptedMikuClient();

class ScriptedMikuClient implements MikuSessionClient {
  ScriptedMikuClient({this.pauseBeforeFinal = false});

  final bool pauseBeforeFinal;
  final Map<String, StreamController<MikuEvent>> _controllers = {};
  final Map<String, MikuSession> _sessions = {};
  final Map<String, DateTime> _updatedAt = {};
  final Map<String, List<SessionMessage>> _messages = {};
  final Map<String, List<MikuEvent>> _pendingEvents = {};
  final Map<String, DriveFeed> _driveFeeds = {};
  final Map<String, String> _approvalSessions = {};
  final Map<String, String> _approvalProposals = {};
  final Map<String, String> _approvalBackends = {};
  final Map<String, Map<String, Object?>> _proposals = {};
  final Map<String, String> rememberedLastEventIds = {};
  final List<String?> eventResumeIds = [];
  final List<String> resolvedApprovals = [];
  final List<String> lockedModes = [];
  final List<String> overriddenModes = [];
  final List<List<String>> promotedResources = [];
  final List<String?> promotedSummaries = [];
  final List<String> sentClientMessageIds = [];
  int driveFeedRequests = 0;
  final Set<String> _acceptedMessageKeys = {};
  final Map<String, String> _pausedFinalTexts = {};
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
    _driveFeeds[id] = DriveFeed(
      recent: const [],
      virtualDirs: _defaultDriveVirtualDirs(),
      proposals: const [],
      pendingApprovals: const [],
    );
    _controllers[id] = StreamController<MikuEvent>.broadcast();
    _currentId = id;
    return session;
  }

  void seedPendingApproval(
    String sessionId, {
    required String approvalId,
    required String action,
    String backend = 'native-tm',
    Map<String, Object?> scope = const {},
    List<Map<String, Object?>> options = const [
      {'optionId': 'allow', 'name': 'Allow once', 'kind': 'allow_once'},
      {'optionId': 'reject', 'name': 'Reject once', 'kind': 'reject_once'},
    ],
  }) {
    _approvalSessions[approvalId] = sessionId;
    _approvalBackends[approvalId] = backend;
    final event = MikuEvent(
      type: 'approval',
      id: _eventId(),
      data: {
        'approvalId': approvalId,
        'backend': backend,
        'action': action,
        'scope': scope,
        'options': options,
        'timeoutMs': 60000,
      },
    );
    _pendingEvents.putIfAbsent(sessionId, () => []).add(event);
  }

  void emitEvent(String sessionId, MikuEvent event) {
    _controllers[sessionId]?.add(event);
  }

  @override
  Future<List<SessionSummary>> listSessions({int limit = 30}) async {
    final ids =
        _sessions.keys.toList()..sort(
          (a, b) => (_updatedAt[b] ?? DateTime.fromMillisecondsSinceEpoch(0))
              .compareTo(
                _updatedAt[a] ?? DateTime.fromMillisecondsSinceEpoch(0),
              ),
        );
    return ids.take(limit).map((id) {
      final session = _sessions[id]!;
      final messages = _messages[id] ?? const [];
      final firstUser =
          messages
              .where((message) => message.role == 'user')
              .map((message) => message.content)
              .firstOrNull;
      final summary =
          messages.reversed
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
      pendingEvents: List<MikuEvent>.from(
        _pendingEvents[session.id] ?? const [],
      ),
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
  Future<void> sendMessage(
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
            'code':
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
      final approvalEvent = MikuEvent(
        type: 'approval',
        id: _eventId(),
        data: {
          'approvalId': approvalId,
          'backend': 'native-tm',
          'action': 'proc.run cargo clean',
          'scope': const {'actorId': 'Worker0', 'capability': 'proc.run'},
          'options': const [
            {'optionId': 'allow', 'name': 'Allow once', 'kind': 'allow_once'},
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
            'shaped':
                'stdout:\ndisplay: scripted actor complete\n\nresult:\nnull',
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
            'voice_cap': 'off',
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
      controller.add(approvalEvent);
      _pendingEvents.putIfAbsent(sessionId, () => []).add(approvalEvent);
    }
  }

  void completePausedTurn({String? sessionId}) {
    final id = sessionId ?? _currentId;
    if (id == null) return;
    final text = _pausedFinalTexts.remove(id);
    if (text == null) return;
    _emitFinal(id, text);
  }

  void _emitFinal(String sessionId, String text) {
    _controllers[sessionId]?.add(
      MikuEvent(type: 'final', id: _eventId(), data: {'text': text}),
    );
    _appendMessage(sessionId, 'assistant', text);
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
    _pendingEvents[sessionId]?.removeWhere(
      (event) =>
          (event.type == 'approval' &&
              event.data['approvalId'] == approvalId) ||
          (proposalId != null &&
              event.type == 'write_proposal' &&
              event.data['proposalId'] == proposalId),
    );
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
  Future<DriveFeed> driveFeed(
    String sessionId, {
    int limit = 20,
    String? project,
  }) async {
    driveFeedRequests++;
    final feed =
        _driveFeeds[sessionId] ??
        DriveFeed(
          recent: const [],
          virtualDirs: _defaultDriveVirtualDirs(),
          proposals: const [],
          pendingApprovals: const [],
        );
    if (project == null || project.trim().isEmpty) return feed;
    final normalized = project.trim().toLowerCase();
    return DriveFeed(
      recent:
          feed.recent
              .where((item) => item.project?.toLowerCase() == normalized)
              .take(limit)
              .toList(),
      virtualDirs: feed.virtualDirs,
      proposals: feed.proposals,
      pendingApprovals: feed.pendingApprovals,
    );
  }

  @override
  Future<ResourcePreview> previewResource(String sessionId, String uri) async {
    if (uri.startsWith('drive://')) {
      return ResourcePreview(
        uri: uri,
        kind: 'drive_document',
        mime: 'text/markdown',
        title: 'Scripted drive note',
        sizeBytes: 128,
        preview: 'Preview for $uri\n\nLocal citation corpus is ready.',
        hasMore: false,
      );
    }
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
  Future<ResourcePreview> resolveResource(String sessionId, String uri) async {
    if (uri.startsWith('drive://')) {
      return ResourcePreview(
        uri: uri,
        kind: 'drive_document',
        mime: 'text/markdown',
        title: 'Scripted drive note',
        sizeBytes: 128,
        preview: 'Preview for $uri\n\nLocal citation corpus is ready.',
        content: '# Scripted drive note\n\nLocal citation corpus is ready.',
        hasMore: false,
      );
    }
    return previewResource(sessionId, uri);
  }

  @override
  Future<ProjectPromotion> promoteSession(
    String sessionId, {
    String? summary,
    List<String> openLoops = const [],
    List<String> decisions = const [],
    List<String> resources = const [],
  }) async {
    promotedSummaries.add(summary);
    promotedResources.add(List<String>.from(resources));
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

  void _emitDriveWorkspace(
    String sessionId,
    StreamController<MikuEvent> controller,
  ) {
    const path = 'projects/tempestmiku/research/p5-drive-workspace.md';
    const uri = 'drive://projects/tempestmiku/research/p5-drive-workspace.md';
    const movedFrom = 'inbox/raw-research.md';
    const movedFromUri = 'drive://inbox/raw-research.md';
    const proposalId = 'drive-proposal-scripted';
    final item = DriveFeedItem(
      uri: uri,
      path: path,
      title: 'P5 drive research notes',
      docKind: 'note',
      project: 'TempestMiku',
      tags: const ['research', 'p5'],
      contentHash: 'sha256:scripted-drive',
      summary: 'Local drive corpus for P5 research with bounded citations.',
      sizeBytes: 128,
      updatedAt: DateTime.now().toIso8601String(),
    );
    const proposal = DriveOrganizerProposal(
      proposalId: proposalId,
      action: 'move',
      status: 'pending',
      sourcePath: movedFrom,
      sourceUri: movedFromUri,
      proposedPath: path,
      proposedUri: uri,
      confidence: 0.91,
      previewTitle: 'Move drive document',
      previewSubtitle:
          'inbox/raw-research.md -> projects/tempestmiku/research/p5-drive-workspace.md',
      previewSnippet: 'Organizer found the project-scoped research note.',
    );
    _driveFeeds[sessionId] = DriveFeed(
      recent: [item],
      virtualDirs: _defaultDriveVirtualDirs(),
      proposals: [proposal],
      pendingApprovals: const [],
    );

    Map<String, Object?> entryPayload(String action, String title) => {
      'action': action,
      'path': path,
      'uri': uri,
      'title': item.title,
      'docKind': item.docKind,
      'project': item.project,
      'tags': item.tags,
      'mime': 'text/markdown',
      'sizeBytes': item.sizeBytes,
      'contentHash': item.contentHash,
      'preview': {'title': title, 'subtitle': path, 'snippet': item.summary},
      'resourceRefs': [
        {
          'role': 'document',
          'uri': uri,
          'kind': 'drive_document',
          'title': item.title,
          'path': path,
        },
      ],
    };

    final proposalPayload = {
      'proposalId': proposalId,
      'action': 'move',
      'status': 'pending',
      'sourcePath': movedFrom,
      'sourceUri': movedFromUri,
      'proposedPath': path,
      'proposedUri': uri,
      'confidence': 0.91,
      'preview': {
        'title': 'Move drive document',
        'subtitle': '$movedFrom -> $path',
        'snippet': 'Organizer found the project-scoped research note.',
      },
      'resourceRefs': [
        {
          'role': 'source',
          'uri': movedFromUri,
          'kind': 'drive_document',
          'title': 'raw-research.md',
        },
        {
          'role': 'proposed',
          'uri': uri,
          'kind': 'drive_document',
          'title': 'p5-drive-workspace.md',
        },
      ],
    };

    controller.add(
      MikuEvent(
        type: 'drive_linked',
        id: _eventId(),
        data: const {
          'action': 'link',
          'alias': 'tempestmiku',
          'linkedUri': 'linked://tempestmiku',
          'mode': 'rw',
          'project': 'TempestMiku',
          'memoryScope': 'project:tempestmiku',
          'preview': {
            'title': 'Linked project folder',
            'subtitle': 'TempestMiku -> linked://tempestmiku',
            'snippet': '/Users/brian/TempestMiku',
          },
          'resourceRefs': [
            {
              'role': 'linked',
              'uri': 'linked://tempestmiku',
              'kind': 'linked_folder',
              'title': 'TempestMiku',
            },
          ],
        },
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_put',
        id: _eventId(),
        data: entryPayload('put', 'Filed drive document'),
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_tagged',
        id: _eventId(),
        data: entryPayload('tag', 'Tagged drive document'),
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_moved',
        id: _eventId(),
        data: {
          ...entryPayload('move', 'Moved drive document'),
          'fromPath': movedFrom,
          'fromUri': movedFromUri,
          'toPath': path,
          'toUri': uri,
        },
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_organizer_started',
        id: _eventId(),
        data: const {
          'apply': false,
          'tier': 'conservative',
          'autoApplyRules': 0,
        },
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_organizer_completed',
        id: _eventId(),
        data: {
          'apply': false,
          'tier': 'conservative',
          'runId': 'scripted-run',
          'proposalCount': 1,
          'proposals': [proposalPayload],
          'resourceRefs': proposalPayload['resourceRefs'],
        },
      ),
    );
  }

  static List<DriveVirtualDir> _defaultDriveVirtualDirs() {
    return const [
      DriveVirtualDir(
        uri: 'drive://recent',
        name: 'recent',
        kind: 'virtual_dir',
        title: 'Recent documents',
      ),
      DriveVirtualDir(
        uri: 'drive://by-project',
        name: 'by-project',
        kind: 'virtual_dir',
        title: 'Documents by project',
      ),
      DriveVirtualDir(
        uri: 'drive://by-type',
        name: 'by-type',
        kind: 'virtual_dir',
        title: 'Documents by type',
      ),
      DriveVirtualDir(
        uri: 'drive://by-tag',
        name: 'by-tag',
        kind: 'virtual_dir',
        title: 'Documents by tag',
      ),
      DriveVirtualDir(
        uri: 'drive://by-date',
        name: 'by-date',
        kind: 'virtual_dir',
        title: 'Documents by date',
      ),
    ];
  }

  MikuSession _sessionForMode(String id, String mode, {bool locked = false}) {
    final lastEventId = _nextEventId > 1 ? '${_nextEventId - 1}' : null;
    return MikuSession(
      id: id,
      mode: mode,
      label: _label(mode),
      voiceCap: _voiceCap(mode),
      defaultScope:
          mode == 'serious_engineer' || mode == 'handoff'
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
      status: session.status,
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
        status: session.status,
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
