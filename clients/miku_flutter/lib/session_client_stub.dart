import 'dart:async';
import 'dart:typed_data';

import 'session_models.dart';

part 'session_client_stub/drive_scenarios.dart';
part 'session_client_stub/message_scenarios.dart';
part 'session_client_stub/mode_catalog.dart';

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
  }) {
    return _sendScriptedMessage(
      sessionId,
      content,
      clientMessageId: clientMessageId,
    );
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
