import 'dart:async';
import 'dart:typed_data';

import 'session_models.dart';

part 'session_client_stub/drive_scenarios.dart';
part 'session_client_stub/message_scenarios.dart';
part 'session_client_stub/mode_catalog.dart';
part 'session_client_stub/proposal_scenarios.dart';

MikuSessionClient createClient() => ScriptedMikuClient();

class ScriptedMikuClient
    implements MikuSessionClient, ServerTargetClient, CurrentAuthDeviceClient {
  ScriptedMikuClient({
    this.pauseBeforeFinal = false,
    this.projectCatalogEmpty = false,
    this.includeArchiveProject = false,
    this.failProjectCatalog = false,
    this.failProjectScope = false,
    this.failProjectResources = false,
    this.failProjectResolve = false,
    this.failDriveFeed = false,
    this.failResourceResolve = false,
  });

  final bool pauseBeforeFinal;
  bool projectCatalogEmpty;
  bool includeArchiveProject;
  bool failProjectCatalog;
  bool failProjectScope;
  bool failProjectResources;
  bool failProjectResolve;
  bool failDriveFeed;
  bool failResourceResolve;
  final Map<String, StreamController<MikuEvent>> _controllers = {};
  final Map<String, MikuSession> _sessions = {};
  final Map<String, DateTime> _updatedAt = {};
  final Map<String, List<SessionMessage>> _messages = {};
  final Map<String, List<MikuEvent>> _pendingEvents = {};
  final Map<String, SessionTurn> _turns = {};
  final Map<String, String> _turnIdsByMessageKey = {};
  final Map<String, String> _activeTurnIds = {};
  final Map<String, String> _pausedTurnIds = {};
  final Map<String, String> _eventTurnIds = {};
  final Map<String, DriveFeed> _driveFeeds = {};
  final Map<String, String> _approvalSessions = {};
  final Map<String, String> _approvalProposals = {};
  final Map<String, String> _approvalProposalEventIds = {};
  final Map<String, String> _approvalBackends = {};
  final Map<String, Map<String, Object?>> _approvalDetails = {};
  final Map<String, Map<String, Object?>> _proposals = {};
  final Map<String, Completer<MemoryWriteProposalResult>>
  _memoryProposalCompleters = {};
  final Map<String, String> rememberedLastEventIds = {};
  final List<String?> eventResumeIds = [];
  final List<String> resolvedApprovals = [];
  final List<String> lockedModes = [];
  final List<String> overriddenModes = [];
  final List<String> assignedProjectIds = [];
  final List<String> assignedSessionIds = [];
  final List<ProjectCatalogEntry> createdProjects = [];
  final List<String> archivedProjectIds = [];
  final List<String> sentClientMessageIds = [];
  final List<String?> resolvedResourceSelectors = [];
  final List<String> revokedDeviceIds = [];
  final List<AuthDevice> _authDevices =
      const [
        AuthDevice(
          id: 'device-current',
          name: 'TempestMiku scripted',
          platform: 'android',
          createdAt: '2026-07-18T10:00:00Z',
          lastSeenAt: '2026-07-20T10:00:00Z',
        ),
        AuthDevice(
          id: 'device-browser',
          name: 'Laptop browser',
          platform: 'web',
          createdAt: '2026-07-19T10:00:00Z',
          lastSeenAt: '2026-07-20T09:00:00Z',
        ),
      ].toList();
  final List<MikuPairingTarget> pairedTargets = [];
  String selectedServerBaseUrl = 'https://miku.example';
  int driveFeedRequests = 0;
  int logoutCount = 0;
  final Set<String> _acceptedMessageKeys = {};
  final Map<String, String> _pausedFinalTexts = {};
  int unlockCount = 0;
  int _nextId = 0;
  int _nextEventId = 1;
  String? _currentId;
  String? _currentAuthDeviceId = 'device-current';
  String? _emittingTurnId;

  @override
  String pairingDeviceName() => 'TempestMiku scripted';

  @override
  Future<String> serverBaseUrl() async => selectedServerBaseUrl;

  @override
  Future<void> setServerBaseUrl(String baseUrl) async {
    final normalized = normalizeMikuServerBaseUrl(baseUrl);
    if (normalized != selectedServerBaseUrl) {
      _currentAuthDeviceId = null;
      selectedServerBaseUrl = normalized;
    }
  }

  @override
  Future<void> pairWithCode(MikuPairingTarget target) async {
    pairedTargets.add(target);
    selectedServerBaseUrl = target.serverBaseUrl;
    _currentAuthDeviceId = 'device-current';
  }

  @override
  Future<String?> currentAuthDeviceId() async => _currentAuthDeviceId;

  @override
  Future<ServerDiagnostics> serverDiagnostics() async {
    return const ServerDiagnostics(
      baseUrl: 'https://miku.example',
      role: 'all',
      postgres: true,
      migrationsApplied: true,
      workersEnabled: true,
      shuttingDown: false,
      turnQueueDepth: 0,
      dreamQueueDepth: 1,
      schedulerQueueDepth: 0,
      approvalEffectQueueDepth: 0,
      pushQueueDepth: 0,
      pendingApprovals: 0,
      leaseReclaims: 0,
      heartbeatFailures: 0,
      linkHydrationFailures: 0,
    );
  }

  @override
  Future<ServerReadiness> serverReadiness() async {
    return ServerReadiness.fromJson(const {
      'status': 'ready',
      'runtime': {
        'role': 'all',
        'postgres': true,
        'migrationsApplied': true,
        'workersEnabled': true,
        'shuttingDown': false,
        'memoryReadiness': {
          'schema': 'ready',
          'pgvector': 'ready',
          'embeddings': 'ready',
        },
      },
      'selfEvolution': {'tier': 'conservative'},
    });
  }

  @override
  Future<List<AuthDevice>> authDevices() async =>
      List.unmodifiable(_authDevices);

  @override
  Future<PairingCode> createPairingCode() async => const PairingCode(
    code: 'aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899',
    pairingLink:
        'tempestmiku://pair?v=1&server=https%3A%2F%2Fmiku.example&code=aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899',
    expiresAt: '2026-07-20T10:05:00Z',
  );

  @override
  Future<void> revokeAuthDevice(String deviceId) async {
    revokedDeviceIds.add(deviceId);
    final index = _authDevices.indexWhere((device) => device.id == deviceId);
    if (index >= 0) {
      final device = _authDevices[index];
      _authDevices[index] = AuthDevice(
        id: device.id,
        name: device.name,
        platform: device.platform,
        createdAt: device.createdAt,
        lastSeenAt: device.lastSeenAt,
        revokedAt: '2026-07-20T10:01:00Z',
      );
    }
  }

  @override
  Future<void> logout() async {
    logoutCount++;
    _currentId = null;
    _currentAuthDeviceId = null;
  }

  @override
  Future<VoiceAsrEngineCatalog> voiceAsrEngines() async =>
      VoiceAsrEngineCatalog.fromJson({
        'engines': const [
          {
            'id': localVoiceAsrEngineId,
            'kind': 'local',
            'label': 'On-device',
            'available': true,
            'maxDurationSeconds': 60,
          },
          {
            'id': selfHostedVoiceAsrEngineId,
            'kind': 'remote',
            'label': 'Home remote (self-hosted)',
            'available': false,
            'modelId': 'not-configured',
            'maxDurationSeconds': 60,
          },
        ],
      });

  @override
  Future<VoiceAsrTranscript> transcribeVoicePcm16({
    required String engineId,
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  }) async {
    validateVoiceAsrPcm16Request(
      engineId: engineId,
      captureId: captureId,
      sampleRate: sampleRate,
      pcm16: pcm16,
    );
    throw UnsupportedError('self-hosted voice ASR is not configured');
  }

  @override
  Future<void> cancelVoiceAsrTranscription() async {}

  @override
  Future<ModeCatalog> modeCatalog() async => _scriptedModeCatalog;

  @override
  Future<MikuSession> createOrReuseSession() async {
    final currentId = _currentId;
    if (currentId != null && _sessions.containsKey(currentId)) {
      return _sessions[currentId]!;
    }
    return createSession();
  }

  @override
  Future<MikuSession> createSession({
    String? projectId,
    MikuMemoryPolicy? memoryPolicy,
  }) async {
    final id = 'scripted-${_nextId++}';
    final now = DateTime.now();
    final base = _sessionForMode(id, 'personal_assistant');
    final session = base.copyWith(
      projectId: projectId,
      memoryPolicy: memoryPolicy ?? MikuMemoryPolicy.global,
    );
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

  @override
  Future<void> endSession(String sessionId) async {
    endSessionForTesting(sessionId);
    _controllers[sessionId]?.add(
      MikuEvent(type: 'session_end', id: _eventId(), data: const {}),
    );
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
    _recordPendingApproval(
      sessionId,
      approvalId: approvalId,
      backend: backend,
      action: action,
      scope: scope,
      options: options,
    );
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

  void endSessionForTesting(String sessionId) {
    final session = _sessions[sessionId];
    if (session == null) throw StateError('unknown session $sessionId');
    _sessions[sessionId] = session.copyWith(status: 'ended');
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
        status: session.status,
        messageCount: messages.length,
        lastEventId: session.lastEventId,
      );
    }).toList();
  }

  @override
  Future<List<ProjectCatalogEntry>> listProjects() async {
    if (failProjectCatalog) throw StateError('project catalog unavailable');
    if (projectCatalogEmpty) return const [];
    return [
      const ProjectCatalogEntry(
        id: 'tempestmiku',
        title: 'TempestMiku',
        status: 'active',
        memoryScope: 'project:tempestmiku',
        defaultMemoryPolicy: MikuMemoryPolicy.project,
        projectUri: 'project://tempestmiku',
        linkedFoldersUri: 'project://tempestmiku/linked-folders',
        linkedFolderUris: ['project://tempestmiku/linked-folders/tempestmiku/'],
      ),
      if (includeArchiveProject)
        const ProjectCatalogEntry(
          id: 'archive',
          title: 'Archive',
          status: 'active',
          memoryScope: 'project:archive',
          defaultMemoryPolicy: MikuMemoryPolicy.project,
          projectUri: 'project://archive',
          linkedFoldersUri: 'project://archive/linked-folders',
        ),
      ...createdProjects,
    ].where((project) => !archivedProjectIds.contains(project.id)).toList();
  }

  @override
  Future<MikuSession> setSessionMemoryContext(
    String sessionId, {
    String? projectId,
    MikuMemoryPolicy? memoryPolicy,
  }) async {
    if (failProjectScope) throw StateError('project scope unavailable');
    final session = _sessions[sessionId];
    if (session == null) throw StateError('unknown session $sessionId');
    if (session.status == 'ended') {
      throw StateError('session $sessionId has ended');
    }
    final updated = session.copyWith(
      projectId: projectId,
      memoryPolicy: memoryPolicy ?? session.memoryPolicy,
    );
    _sessions[sessionId] = updated;
    return updated;
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
        .stream
        .map((event) {
          final turnId =
              event.turnId ??
              (event.id == null ? null : _eventTurnIds[event.id!]);
          if (turnId == event.turnId) return event;
          return MikuEvent(
            type: event.type,
            id: event.id,
            data: event.data,
            turnId: turnId,
            createdAt: event.createdAt,
          );
        });
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {
    rememberedLastEventIds[sessionId] = lastEventId;
  }

  @override
  Future<TurnReceipt> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) async {
    final messageKey = '$sessionId\u{0}$clientMessageId';
    final duplicateTurnId = _turnIdsByMessageKey[messageKey];
    if (duplicateTurnId != null) {
      sentClientMessageIds.add(clientMessageId);
      final duplicate = _turns[duplicateTurnId]!;
      return TurnReceipt(
        turnId: duplicate.id,
        clientMessageId: duplicate.clientMessageId,
        status: duplicate.status,
      );
    }

    final now = DateTime.now().toUtc().toIso8601String();
    final turnId = 'scripted-turn-${_nextId++}';
    _turnIdsByMessageKey[messageKey] = turnId;
    _activeTurnIds[sessionId] = turnId;
    _emittingTurnId = turnId;
    _turns[turnId] = SessionTurn.fromJson({
      'id': turnId,
      'sessionId': sessionId,
      'clientMessageId': clientMessageId,
      'content': content,
      'contentHash': 'scripted:$clientMessageId',
      'status': 'queued',
      'createdAt': now,
      'updatedAt': now,
    });

    try {
      await _sendScriptedMessage(
        sessionId,
        content,
        clientMessageId: clientMessageId,
      );
    } catch (error) {
      final failedAt = DateTime.now().toUtc().toIso8601String();
      _turns[turnId] = SessionTurn.fromJson({
        'id': turnId,
        'sessionId': sessionId,
        'clientMessageId': clientMessageId,
        'content': content,
        'contentHash': 'scripted:$clientMessageId',
        'status': 'failed',
        'createdAt': now,
        'updatedAt': failedAt,
        'startedAt': now,
        'completedAt': failedAt,
        'error': error.toString(),
      });
      _activeTurnIds.remove(sessionId);
      if (_emittingTurnId == turnId) _emittingTurnId = null;
      rethrow;
    }

    final updatedAt = DateTime.now().toUtc().toIso8601String();
    final status = pauseBeforeFinal ? 'running' : 'completed';
    _turns[turnId] = SessionTurn.fromJson({
      'id': turnId,
      'sessionId': sessionId,
      'clientMessageId': clientMessageId,
      'content': content,
      'contentHash': 'scripted:$clientMessageId',
      'status': status,
      'createdAt': now,
      'updatedAt': updatedAt,
      'startedAt': now,
      if (!pauseBeforeFinal) 'completedAt': updatedAt,
    });
    if (pauseBeforeFinal) {
      _pausedTurnIds[sessionId] = turnId;
    } else {
      _activeTurnIds.remove(sessionId);
    }
    if (_emittingTurnId == turnId) _emittingTurnId = null;
    return TurnReceipt(
      turnId: turnId,
      clientMessageId: clientMessageId,
      status: status,
    );
  }

  @override
  Future<SessionTurn> getTurn(String sessionId, String turnId) async {
    final turn = _turns[turnId];
    if (turn == null || turn.sessionId != sessionId) {
      throw StateError('unknown turn $turnId for session $sessionId');
    }
    return turn;
  }

  @override
  Future<MemoryWriteProposalResult> proposeMemoryWrite(
    String sessionId,
    MemoryWriteProposalRequest request,
  ) => _proposeMemoryWriteImpl(sessionId, request);

  @override
  Future<EvolutionReviewProposalResult> proposeEvolutionReview(
    String sessionId,
    EvolutionReviewProposalRequest request,
  ) => _proposeEvolutionReviewImpl(sessionId, request);

  @override
  Future<ModeAddendumRollbackResult> proposeModeAddendumRollback(
    String sessionId,
    String modeId,
    AddendumRollbackRequest request,
  ) => _proposeModeAddendumRollbackImpl(sessionId, modeId, request);

  @override
  Future<PersonaAddendumRollbackResult> proposePersonaAddendumRollback(
    String sessionId,
    String personaId,
    AddendumRollbackRequest request,
  ) => _proposePersonaAddendumRollbackImpl(sessionId, personaId, request);

  @override
  Future<SkillRollbackResult> proposeSkillRollback(
    String sessionId,
    String skillName,
    SkillRollbackRequest request,
  ) => _proposeSkillRollbackImpl(sessionId, skillName, request);

  @override
  Future<ApprovalDetails> getApproval(String sessionId, String approvalId) =>
      _getApprovalImpl(sessionId, approvalId);

  void completePausedTurn({String? sessionId}) {
    final id = sessionId ?? _currentId;
    if (id == null) return;
    final text = _pausedFinalTexts.remove(id);
    if (text == null) return;
    _emitFinal(id, text);
    final turnId = _pausedTurnIds.remove(id);
    final turn = turnId == null ? null : _turns[turnId];
    if (turn != null) {
      final completedAt = DateTime.now().toUtc().toIso8601String();
      _turns[turn.id] = SessionTurn.fromJson({
        'id': turn.id,
        'sessionId': turn.sessionId,
        'clientMessageId': turn.clientMessageId,
        'content': turn.content,
        'contentHash': turn.contentHash,
        'status': 'completed',
        'createdAt': turn.createdAt,
        'updatedAt': completedAt,
        if (turn.startedAt != null) 'startedAt': turn.startedAt,
        'completedAt': completedAt,
        if (turn.workerId != null) 'workerId': turn.workerId,
      });
    }
    _activeTurnIds.remove(id);
  }

  void _emitFinal(String sessionId, String text) {
    _controllers[sessionId]?.add(
      MikuEvent(
        type: 'final',
        id: _eventId(),
        data: {'text': text},
        turnId: _activeTurnIds[sessionId],
        createdAt: DateTime.now().toUtc().toIso8601String(),
      ),
    );
    _appendMessage(sessionId, 'assistant', text);
  }

  @override
  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision, {
    String? optionId,
  }) {
    return _resolveScriptedApproval(
      sessionId,
      approvalId,
      decision,
      optionId: optionId,
    );
  }

  @override
  Future<void> lockMode(String sessionId, String mode) async {
    final session = _sessionForMode(sessionId, mode, locked: true);
    lockedModes.add(mode);
    _sessions[sessionId] = session;
    _controllers[sessionId]?.add(
      MikuEvent(
        type: 'mode',
        data: {
          'mode': mode,
          'label': session.label,
          'activeSkills': session.activeSkills,
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
    final current = _sessions[sessionId];
    final mode = current?.mode ?? 'personal_assistant';
    _sessions[sessionId] = _sessionForMode(sessionId, mode);
    _controllers[sessionId]?.add(
      MikuEvent(
        type: 'mode',
        data: {
          'mode': mode,
          'label': _label(mode),
          'activeSkills': _activeSkills(mode),
        },
      ),
    );
  }

  @override
  Future<void> overrideMode(String sessionId, String mode) async {
    final session = _sessionForMode(sessionId, mode);
    overriddenModes.add(mode);
    _sessions[sessionId] = session;
    _controllers[sessionId]?.add(
      MikuEvent(
        type: 'mode',
        data: {
          'mode': mode,
          'label': session.label,
          'activeSkills': session.activeSkills,
          'override_source': 'user',
        },
      ),
    );
  }

  @override
  Future<ProjectOverview> projectOverview(String sessionId) async {
    return const ProjectOverview(
      projectId: 'tempestmiku',
      projectUri: 'project://tempestmiku',
      status: 'Scripted project status',
      nextActions: ['Continue from latest session result'],
      openLoops: [
        ProjectItem(
          id: 'scripted-open-loop',
          kind: 'open_loop',
          text: 'Verify the next UI slice',
          targetUri: 'project://tempestmiku/open-loops/scripted',
        ),
      ],
      decisions: [
        ProjectItem(
          id: 'scripted-decision',
          kind: 'decision',
          text: 'Keep chat as the primary surface',
          targetUri: 'project://tempestmiku/decisions/scripted',
        ),
      ],
    );
  }

  @override
  Future<DriveFeed> driveFeed(
    String sessionId, {
    int limit = 20,
    String? project,
  }) async {
    driveFeedRequests++;
    if (failDriveFeed) throw StateError('drive feed unavailable');
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
    if (uri == 'artifact://scripted-report') {
      return const ResourcePreview(
        uri: 'artifact://scripted-report',
        kind: 'text',
        mime: 'text/plain',
        title: 'Scripted report',
        sizeBytes: 4096,
        preview: 'Preview for artifact://scripted-report (compact)',
        hasMore: true,
      );
    }
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
  Future<ResourcePreview> resolveResource(
    String sessionId,
    String uri, {
    String? selector,
  }) async {
    resolvedResourceSelectors.add(selector);
    if (failResourceResolve) {
      throw StateError('resource page unavailable');
    }
    if (failProjectResolve) throw StateError('project resource unavailable');
    if (uri == 'artifact://scripted-report' && selector != null) {
      return ResourcePreview(
        uri: uri,
        kind: 'text',
        mime: 'text/plain',
        title: 'Scripted report',
        sizeBytes: 4096,
        preview: 'Resolved preview for $selector',
        content:
            selector == '1-200'
                ? 'Resolved lines 1-200'
                : 'Resolved lines 201-400',
        selector: selector,
        hasMore: selector == '1-200',
      );
    }
    if (uri.startsWith('project://tempestmiku/linked-folders/')) {
      return ResourcePreview(
        uri: uri,
        kind: 'text',
        mime: 'text/plain',
        title: uri.split('/').last,
        sizeBytes: 96,
        preview: 'Scripted linked resource for $uri',
        content: 'Scripted linked resource for $uri',
        hasMore: uri.endsWith('README.md'),
      );
    }
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
  Future<List<MikuResourceEntry>> listResources(
    String sessionId,
    String uri,
  ) async {
    if (failProjectResources) throw StateError('project listing unavailable');
    if (uri.isEmpty) {
      return const [
        MikuResourceEntry(uri: 'artifact://', name: 'artifact', kind: 'scheme'),
        MikuResourceEntry(uri: 'memory://', name: 'memory', kind: 'scheme'),
        MikuResourceEntry(uri: 'agent://', name: 'agent', kind: 'scheme'),
        MikuResourceEntry(uri: 'history://', name: 'history', kind: 'scheme'),
        MikuResourceEntry(uri: 'skill://', name: 'skill', kind: 'scheme'),
      ];
    }
    if (uri == 'artifact://') {
      return const [
        MikuResourceEntry(
          uri: 'artifact://scripted-report',
          name: 'scripted-report',
          kind: 'text',
          title: 'Scripted report',
          sizeBytes: 48,
        ),
      ];
    }
    if (uri == 'memory://' || uri == 'agent://') return const [];
    if (uri == 'skill://') {
      return const [
        MikuResourceEntry(
          uri: 'skill://scripted-skill',
          name: 'scripted-skill',
          kind: 'managed_skill',
          title: 'Scripted managed skill',
        ),
      ];
    }
    if (uri == 'skill://scripted-skill') {
      return const [
        MikuResourceEntry(
          uri:
              'skill://scripted-skill/versions/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
          name:
              'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
          kind: 'managed_skill_version',
          title: 'Earlier scripted version',
        ),
      ];
    }
    final projectId = _sessions[sessionId]?.projectId;
    if (projectId != 'tempestmiku') {
      throw StateError('404 active project scope');
    }
    return switch (uri) {
      'project://tempestmiku/linked-folders' => const [
        MikuResourceEntry(
          uri: 'project://tempestmiku/linked-folders/tempestmiku/',
          name: 'tempestmiku',
          kind: 'linked_folder',
          title: 'linked://tempestmiku/',
        ),
      ],
      'project://tempestmiku/linked-folders/tempestmiku/' => const [
        MikuResourceEntry(
          uri: 'project://tempestmiku/linked-folders/tempestmiku/docs/',
          name: 'docs',
          kind: 'dir',
        ),
        MikuResourceEntry(
          uri: 'project://tempestmiku/linked-folders/tempestmiku/README.md',
          name: 'README.md',
          kind: 'file',
          sizeBytes: 96,
        ),
        MikuResourceEntry(
          uri: 'project://tempestmiku/linked-folders/tempestmiku/latest',
          name: 'latest',
          kind: 'symlink',
        ),
      ],
      'project://tempestmiku/linked-folders/tempestmiku/docs/' => const [
        MikuResourceEntry(
          uri: 'project://tempestmiku/linked-folders/tempestmiku/docs/guide.md',
          name: 'guide.md',
          kind: 'file',
          sizeBytes: 64,
        ),
      ],
      _ => const <MikuResourceEntry>[],
    };
  }

  @override
  Future<int> assignSessionToProject(String projectId, String sessionId) async {
    assignedProjectIds.add(projectId);
    assignedSessionIds.add(sessionId);
    return 3;
  }

  @override
  Future<ProjectCatalogEntry> createProject(
    String id, {
    String? title,
    MikuMemoryPolicy? defaultMemoryPolicy,
  }) async {
    final slug = id.trim().toLowerCase().replaceAll(RegExp(r'[^a-z0-9]+'), '-');
    final entry = ProjectCatalogEntry(
      id: slug,
      title: title ?? id,
      status: 'active',
      memoryScope: 'project:$slug',
      defaultMemoryPolicy: defaultMemoryPolicy ?? MikuMemoryPolicy.project,
      projectUri: 'project://$slug',
      linkedFoldersUri: 'project://$slug/linked-folders',
    );
    createdProjects.removeWhere((project) => project.id == slug);
    archivedProjectIds.remove(slug);
    createdProjects.add(entry);
    return entry;
  }

  @override
  Future<ProjectCatalogEntry> archiveProject(
    String projectId, {
    String? reason,
  }) async {
    archivedProjectIds.add(projectId);
    final existing =
        createdProjects.where((project) => project.id == projectId).firstOrNull;
    return ProjectCatalogEntry(
      id: projectId,
      title: existing?.title ?? projectId,
      status: 'archived',
      memoryScope: 'project:$projectId',
      defaultMemoryPolicy:
          existing?.defaultMemoryPolicy ?? MikuMemoryPolicy.project,
      projectUri: 'project://$projectId',
      linkedFoldersUri: 'project://$projectId/linked-folders',
    );
  }

  String _label(String mode) {
    return switch (mode) {
      'personal_assistant' => 'Personal Assistant',
      'serious_engineer' => 'Serious Engineer',
      'ambiguity_grill' => 'Ambiguity Grill',
      'negative_state_grounding' => 'Negative-State Grounding',
      _ => throw ArgumentError.value(mode, 'mode', 'unknown mode'),
    };
  }

  List<String> _activeSkills(String mode) {
    return switch (mode) {
      'personal_assistant' => const [
        'miku-voice',
        'personal-assistant-state-capture',
      ],
      'ambiguity_grill' => const ['miku-voice', 'ambiguity-grill'],
      'negative_state_grounding' => const [
        'miku-voice',
        'negative-state-grounding',
      ],
      'serious_engineer' => const [],
      _ => throw ArgumentError.value(mode, 'mode', 'unknown mode'),
    };
  }

  MikuSession _sessionForMode(String id, String mode, {bool locked = false}) {
    final lastEventId = _nextEventId > 1 ? '${_nextEventId - 1}' : null;
    return MikuSession(
      id: id,
      mode: mode,
      label: _label(mode),
      activeSkills: _activeSkills(mode),
      locked: locked,
      lastEventId: lastEventId,
    );
  }

  MikuSession _copySession(MikuSession session, {String? lastEventId}) =>
      session.copyWith(lastEventId: lastEventId);

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
      _sessions[sessionId] = session.copyWith(
        lastEventId: '${_nextEventId - 1}',
      );
    }
  }

  String _eventId() {
    final id = '${_nextEventId++}';
    final turnId = _emittingTurnId;
    if (turnId != null) _eventTurnIds[id] = turnId;
    return id;
  }
}
