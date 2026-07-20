import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:flutter/foundation.dart' show visibleForTesting;
import 'package:web/web.dart' as web;

import 'session_models.dart';
import 'session_sse.dart';

MikuSessionClient createClient() => WebMikuSessionClient();

class _AmbiguousWebTransportFailure implements Exception {
  const _AmbiguousWebTransportFailure();
}

class WebMikuSessionClient
    implements MikuSessionClient, ServerTargetClient, CurrentAuthDeviceClient {
  WebMikuSessionClient({this.pairRequestForTesting});

  static const _sessionIdKey = 'tempestmiku.sessionId';
  static const _lastEventIdKey = 'tempestmiku.lastEventId';
  static const _currentDeviceIdKey = 'tempestmiku.currentAuthDeviceId';
  static const _currentDeviceOriginKey = 'tempestmiku.currentAuthDeviceOrigin';

  @visibleForTesting
  final Future<Map<String, Object?>> Function(MikuPairingTarget target)?
  pairRequestForTesting;

  @override
  String pairingDeviceName() => 'TempestMiku web';

  @override
  Future<String> serverBaseUrl() async => web.window.location.origin;

  @override
  Future<void> setServerBaseUrl(String baseUrl) async {
    final normalized = normalizeMikuServerBaseUrl(baseUrl);
    if (normalized != web.window.location.origin) {
      throw UnsupportedError(
        'the Web client can pair only with its current server origin',
      );
    }
  }

  @override
  Future<void> pairWithCode(MikuPairingTarget target) async {
    await setServerBaseUrl(target.serverBaseUrl);
    late final PairedAuthDeviceIdentity identity;
    try {
      final requestOverride = pairRequestForTesting;
      final json =
          requestOverride == null
              ? await _request(
                'POST',
                '/auth/pair',
                body: {
                  'code': target.code,
                  'deviceName': pairingDeviceName(),
                  'platform': 'web',
                },
              )
              : await requestOverride(target);
      identity = PairedAuthDeviceIdentity.fromPairResponse(
        json,
        serverBaseUrl: target.serverBaseUrl,
      );
    } on StateError {
      // `_request` uses StateError for a definitive non-2xx response. The
      // server does not rotate the auth cookie on a rejected pairing code, so
      // retain the still-valid local marker for the existing authority.
      rethrow;
    } catch (_) {
      // A successful HTTP response may already have rotated the HttpOnly
      // cookie even when its body cannot be decoded. Never associate that
      // unknown authority with the previously paired device row.
      _forgetCurrentAuthDevice();
      rethrow;
    }
    web.window.localStorage.removeItem(_sessionIdKey);
    web.window.localStorage.removeItem(_lastEventIdKey);
    _rememberCurrentAuthDevice(identity);
  }

  @override
  Future<ServerDiagnostics> serverDiagnostics() async {
    final json = await _request('GET', '/metrics');
    return ServerDiagnostics.fromJson(json, baseUrl: await serverBaseUrl());
  }

  @override
  Future<ServerReadiness> serverReadiness() async {
    late final web.Response response;
    try {
      response = await _fetch(
        'GET',
        '/ready',
        headers: const {'accept': 'application/json'},
      );
    } catch (_) {
      throw const _AmbiguousWebTransportFailure();
    }
    final status = response.status;
    if (status == 0) throw const _AmbiguousWebTransportFailure();
    final text = await _responseText(response);
    if (status != 200 && status != 503) {
      throw StateError('request failed: $status $text');
    }
    if (text.isEmpty) {
      throw const FormatException('server readiness response was empty');
    }
    final decoded = jsonDecode(text);
    if (decoded is! Map) {
      throw const FormatException(
        'server returned a non-object readiness response',
      );
    }
    return ServerReadiness.fromJson(decoded.cast<String, Object?>());
  }

  @override
  Future<List<AuthDevice>> authDevices() async {
    final json = await _request('GET', '/auth/devices');
    return ((json['devices'] as List?) ?? const [])
        .whereType<Map>()
        .map((item) => AuthDevice.fromJson(item.cast<String, Object?>()))
        .toList();
  }

  @override
  Future<PairingCode> createPairingCode() async {
    final json = await _request('POST', '/auth/pairing-codes');
    return PairingCode.fromJson(json);
  }

  @override
  Future<void> revokeAuthDevice(String deviceId) async {
    await _request('DELETE', '/auth/devices/$deviceId');
  }

  @override
  Future<void> logout() async {
    try {
      await _request('POST', '/auth/logout');
    } finally {
      web.window.localStorage.removeItem(_sessionIdKey);
      web.window.localStorage.removeItem(_lastEventIdKey);
      _forgetCurrentAuthDevice();
    }
  }

  @override
  Future<String?> currentAuthDeviceId() async {
    final identity = PairedAuthDeviceIdentity.fromStored(
      serverBaseUrl: web.window.localStorage.getItem(_currentDeviceOriginKey),
      deviceId: web.window.localStorage.getItem(_currentDeviceIdKey),
    );
    if (identity == null ||
        !identity.matchesServer(web.window.location.origin)) {
      _forgetCurrentAuthDevice();
      return null;
    }
    return identity.deviceId;
  }

  void _rememberCurrentAuthDevice(PairedAuthDeviceIdentity identity) {
    // The id is the publication point. A crash after writing only the origin
    // leaves the identity unknown instead of associating an old id with it.
    web.window.localStorage.removeItem(_currentDeviceIdKey);
    web.window.localStorage.setItem(
      _currentDeviceOriginKey,
      identity.serverBaseUrl,
    );
    web.window.localStorage.setItem(_currentDeviceIdKey, identity.deviceId);
  }

  void _forgetCurrentAuthDevice() {
    web.window.localStorage.removeItem(_currentDeviceIdKey);
    web.window.localStorage.removeItem(_currentDeviceOriginKey);
  }

  @override
  Future<VoiceAsrEngineCatalog> voiceAsrEngines() async {
    final json = await _request('GET', '/voice/asr/engines');
    return VoiceAsrEngineCatalog.fromJson(json);
  }

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
    late final web.Response response;
    try {
      response = await _fetch(
        'POST',
        '/voice/asr/transcriptions',
        headers: {
          'content-type': 'application/octet-stream',
          'accept': 'application/json',
          'x-tm-asr-engine-id': engineId,
          'x-tm-capture-id': captureId,
          'x-tm-sample-rate': '$sampleRate',
          'x-tm-channels': '$voiceAsrChannels',
        },
        body: pcm16.toJS,
      );
    } catch (_) {
      throw const _AmbiguousWebTransportFailure();
    }
    final status = response.status;
    if (status == 0) throw const _AmbiguousWebTransportFailure();
    if (status < 200 || status >= 300) {
      throw StateError(
        'request failed: $status ${await _responseText(response)}',
      );
    }
    final text = await _responseText(response);
    if (text.isEmpty) {
      throw const FormatException('voice ASR returned an empty response');
    }
    return VoiceAsrTranscript.fromJson(
      (jsonDecode(text) as Map).cast<String, Object?>(),
    );
  }

  @override
  Future<void> cancelVoiceAsrTranscription() async {
    // Browser voice capture is currently unsupported. Keep the shared client
    // contract explicit without inventing a second web cancellation path.
  }

  @override
  Future<ModeCatalog> modeCatalog() async {
    final json = await _request('GET', '/modes');
    return ModeCatalog.fromJson(json);
  }

  @override
  Future<MikuSession> createOrReuseSession() async {
    final storedId = web.window.localStorage.getItem(_sessionIdKey);
    if (storedId != null && storedId.isNotEmpty) {
      try {
        final json = await _request('GET', '/sessions/$storedId');
        final session = _sessionFromJson(
          json,
          lastEventId: web.window.localStorage.getItem(_lastEventIdKey),
        );
        _rememberSession(session);
        return session;
      } catch (_) {
        web.window.localStorage.removeItem(_sessionIdKey);
        web.window.localStorage.removeItem(_lastEventIdKey);
      }
    }
    return createSession();
  }

  @override
  Future<MikuSession> createSession() async {
    final json = await _request('POST', '/sessions');
    final session = _sessionFromJson(json);
    _rememberSession(session);
    return session;
  }

  @override
  Future<void> endSession(String sessionId) async {
    await _request('POST', '/sessions/$sessionId/end');
  }

  @override
  Future<List<SessionSummary>> listSessions({int limit = 30}) async {
    final query = Uri(queryParameters: {'limit': '$limit'}).query;
    final json = await _request('GET', '/sessions?$query');
    return ((json['sessions'] as List?) ?? const [])
        .whereType<Map>()
        .map((item) => _summaryFromJson(item.cast<String, Object?>()))
        .toList();
  }

  @override
  Future<List<ProjectCatalogEntry>> listProjects() async {
    final json = await _request('GET', '/projects');
    return ((json['projects'] as List?) ?? const [])
        .whereType<Map>()
        .map(
          (item) => ProjectCatalogEntry.fromJson(item.cast<String, Object?>()),
        )
        .toList();
  }

  @override
  Future<ProjectCatalogEntry> createProject(String id, {String? title}) async {
    final json = await _request(
      'POST',
      '/projects',
      body: {'id': id, if (title != null) 'title': title},
    );
    return ProjectCatalogEntry.fromJson(json);
  }

  @override
  Future<ProjectCatalogEntry> archiveProject(
    String projectId, {
    String? reason,
  }) async {
    final json = await _request(
      'POST',
      '/projects/$projectId/archive',
      body: reason == null ? const {} : {'reason': reason},
    );
    return ProjectCatalogEntry.fromJson(json);
  }

  @override
  Future<String> setSessionScope(String sessionId, String scope) async {
    final json = await _request(
      'POST',
      '/sessions/$sessionId/scope',
      body: {'scope': scope},
    );
    return (json['memoryScope'] as String?) ??
        (json['memory_scope'] as String?) ??
        scope;
  }

  @override
  Future<LoadedSession> loadSession(String sessionId) async {
    final json = await _request('GET', '/sessions/$sessionId/messages');
    final lastEventId = _nullableString(
      json['lastEventId'] ?? json['last_event_id'],
    );
    final session = _sessionFromJson(json, lastEventId: lastEventId);
    _rememberSession(session);
    final messages =
        ((json['messages'] as List?) ?? const [])
            .whereType<Map>()
            .map((item) => _messageFromJson(item.cast<String, Object?>()))
            .toList();
    final pendingEvents =
        ((json['pendingEvents'] as List?) ??
                (json['pending_events'] as List?) ??
                const [])
            .whereType<Map>()
            .map((item) => _eventFromJson(item.cast<String, Object?>()))
            .toList();
    return LoadedSession(
      session: session,
      messages: messages,
      pendingEvents: pendingEvents,
    );
  }

  @override
  Stream<MikuEvent> events(String sessionId, {String? lastEventId}) {
    final controller = StreamController<MikuEvent>();
    var closed = false;
    web.AbortController? activeAbortController;
    controller.onCancel = () {
      closed = true;
      final abortController = activeAbortController;
      abortController?.abort();
    };
    unawaited(
      _pumpEvents(
        controller,
        () => closed || controller.isClosed,
        (value) => activeAbortController = value,
        sessionId,
        lastEventId,
      ),
    );
    return controller.stream;
  }

  Future<void> _pumpEvents(
    StreamController<MikuEvent> controller,
    bool Function() isClosed,
    void Function(web.AbortController? controller) setAbortController,
    String sessionId,
    String? initialLastEventId,
  ) async {
    var resumeId =
        initialLastEventId ?? web.window.localStorage.getItem(_lastEventIdKey);
    if (numericEventId(resumeId) == null) resumeId = null;
    final lifecycle = SessionEventLifecycle(resumeId);

    while (!isClosed() && lifecycle.shouldReconnect) {
      try {
        final abortController = web.AbortController();
        setAbortController(abortController);
        final decoder = SessionEventSseDecoder();
        eventStream:
        await for (final chunk in _fetchEventChunks(
          sessionId,
          resumeId,
          abortController,
          () {
            if (!isClosed()) {
              controller.add(
                const MikuEvent(
                  type: 'connection',
                  data: {'status': 'connected'},
                ),
              );
            }
          },
        )) {
          for (final event in decoder.add(chunk)) {
            if (isClosed()) break;
            if (!lifecycle.accept(event)) continue;
            final eventId = event.id!;
            if (shouldRememberEventId(event.type, event.data)) {
              resumeId = eventId;
              rememberLastEventId(sessionId, eventId);
            }
            controller.add(event);
            if (lifecycle.isTerminal) break eventStream;
          }
          if (isClosed()) break;
        }
        if (!isClosed() && !lifecycle.isTerminal) {
          for (final event in decoder.close()) {
            if (!lifecycle.accept(event)) continue;
            final eventId = event.id!;
            if (shouldRememberEventId(event.type, event.data)) {
              resumeId = eventId;
              rememberLastEventId(sessionId, eventId);
            }
            controller.add(event);
            if (lifecycle.isTerminal) break;
          }
        }
      } catch (_) {
        if (!isClosed() && lifecycle.shouldReconnect) {
          controller.add(
            const MikuEvent(
              type: 'connection',
              data: {'status': 'reconnecting'},
            ),
          );
          await Future<void>.delayed(const Duration(seconds: 2));
        }
      } finally {
        setAbortController(null);
      }
    }
    if (lifecycle.isTerminal && !controller.isClosed) {
      await controller.close();
    }
  }

  Stream<String> _fetchEventChunks(
    String sessionId,
    String? lastEventId,
    web.AbortController abortController,
    void Function() onOpen,
  ) async* {
    final suffix =
        lastEventId == null
            ? ''
            : '?lastEventId=${Uri.encodeQueryComponent(lastEventId)}';
    final headers =
        web.Headers()
          ..set('accept', 'text/event-stream')
          ..set('cache-control', 'no-cache');
    if (lastEventId != null) headers.set('Last-Event-ID', lastEventId);
    final response =
        await web.window
            .fetch(
              '/sessions/$sessionId/events$suffix'.toJS,
              web.RequestInit(
                method: 'GET',
                credentials: 'same-origin',
                cache: 'no-store',
                headers: headers,
                signal: abortController.signal,
              ),
            )
            .toDart;
    final status = response.status;
    if (status < 200 || status >= 300) {
      throw StateError('event stream failed: $status');
    }
    final body = response.body;
    if (body == null) throw StateError('streaming fetch body is unavailable');
    onOpen();
    final reader = body.getReader() as web.ReadableStreamDefaultReader;
    final textDecoder = web.TextDecoder('utf-8');
    while (true) {
      final result = await reader.read().toDart;
      if (result.done) break;
      final value = result.value;
      if (value == null) continue;
      final chunk = textDecoder.decode(
        value as JSObject,
        web.TextDecodeOptions(stream: true),
      );
      if (chunk.isNotEmpty) yield chunk;
    }
    final tail = textDecoder.decode();
    if (tail.isNotEmpty) yield tail;
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {
    if (web.window.localStorage.getItem(_sessionIdKey) == sessionId) {
      web.window.localStorage.setItem(_lastEventIdKey, lastEventId);
    }
  }

  @override
  Future<TurnReceipt> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) {
    return sendIdempotentMessageWithRetry<TurnReceipt>(
      clientMessageId: clientMessageId,
      isAmbiguousFailure: (error) => error is _AmbiguousWebTransportFailure,
      send: (stableClientMessageId) async {
        final json = await _request(
          'POST',
          '/sessions/$sessionId/messages',
          body: {'clientMessageId': stableClientMessageId, 'content': content},
        );
        return TurnReceipt.fromJson(json);
      },
    );
  }

  @override
  Future<SessionTurn> getTurn(String sessionId, String turnId) async {
    final json = await _request('GET', '/sessions/$sessionId/turns/$turnId');
    return SessionTurn.fromJson(json);
  }

  @override
  Future<MemoryWriteProposalResult> proposeMemoryWrite(
    String sessionId,
    MemoryWriteProposalRequest request,
  ) async {
    final json = await _request(
      'POST',
      '/sessions/$sessionId/memory/proposals',
      body: request.toJson(),
    );
    return MemoryWriteProposalResult.fromJson(json);
  }

  @override
  Future<EvolutionReviewProposalResult> proposeEvolutionReview(
    String sessionId,
    EvolutionReviewProposalRequest request,
  ) async {
    final json = await _request(
      'POST',
      '/sessions/$sessionId/evolution/review-proposals',
      body: request.toJson(),
    );
    return EvolutionReviewProposalResult.fromJson(json);
  }

  @override
  Future<ModeAddendumRollbackResult> proposeModeAddendumRollback(
    String sessionId,
    String modeId,
    AddendumRollbackRequest request,
  ) async {
    final name = Uri.encodeComponent(modeId);
    final json = await _request(
      'POST',
      '/sessions/$sessionId/evolution/modes/$name/rollback',
      body: request.toJson(),
    );
    return ModeAddendumRollbackResult.fromJson(json);
  }

  @override
  Future<PersonaAddendumRollbackResult> proposePersonaAddendumRollback(
    String sessionId,
    String personaId,
    AddendumRollbackRequest request,
  ) async {
    final name = Uri.encodeComponent(personaId);
    final json = await _request(
      'POST',
      '/sessions/$sessionId/evolution/personas/$name/rollback',
      body: request.toJson(),
    );
    return PersonaAddendumRollbackResult.fromJson(json);
  }

  @override
  Future<SkillRollbackResult> proposeSkillRollback(
    String sessionId,
    String skillName,
    SkillRollbackRequest request,
  ) async {
    final name = Uri.encodeComponent(skillName);
    final json = await _request(
      'POST',
      '/sessions/$sessionId/evolution/skills/$name/rollback',
      body: request.toJson(),
    );
    return SkillRollbackResult.fromJson(json);
  }

  @override
  Future<ApprovalDetails> getApproval(
    String sessionId,
    String approvalId,
  ) async {
    final json = await _request(
      'GET',
      '/sessions/$sessionId/approvals/$approvalId',
    );
    return ApprovalDetails.fromJson(json);
  }

  @override
  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision, {
    String? optionId,
  }) async {
    await _request(
      'POST',
      '/sessions/$sessionId/approvals/$approvalId',
      body: {'decision': decision, if (optionId != null) 'optionId': optionId},
    );
  }

  @override
  Future<void> lockMode(String sessionId, String mode) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/lock',
      body: {'mode': mode, 'reason': 'flutter lock'},
    );
  }

  @override
  Future<void> unlockMode(String sessionId) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/unlock',
      body: {'reason': 'flutter unlock'},
    );
  }

  @override
  Future<void> overrideMode(String sessionId, String mode) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/override',
      body: {'mode': mode, 'reason': 'flutter override'},
    );
  }

  @override
  Future<ProjectOverview> projectOverview(String sessionId) async {
    final json = await _request('GET', '/sessions/$sessionId/project');
    return ProjectOverview.fromJson(json);
  }

  @override
  Future<DriveFeed> driveFeed(
    String sessionId, {
    int limit = 20,
    String? project,
  }) async {
    final trimmedProject = project?.trim();
    final query =
        Uri(
          queryParameters: {
            'limit': '$limit',
            if (trimmedProject != null && trimmedProject.isNotEmpty)
              'project': trimmedProject,
          },
        ).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/drive/feed?$query',
    );
    return DriveFeed.fromJson(json);
  }

  @override
  Future<ResourcePreview> previewResource(String sessionId, String uri) async {
    final query = Uri(queryParameters: {'uri': uri}).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/resources/preview?$query',
    );
    return _resourcePreviewFromJson(json, uri);
  }

  @override
  Future<ResourcePreview> resolveResource(
    String sessionId,
    String uri, {
    String? selector,
  }) async {
    final query =
        Uri(
          queryParameters: {
            'uri': uri,
            if (selector != null) 'selector': selector,
          },
        ).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/resources/resolve?$query',
    );
    return _resourcePreviewFromJson(json, uri);
  }

  @override
  Future<List<MikuResourceEntry>> listResources(
    String sessionId,
    String uri,
  ) async {
    final query = Uri(queryParameters: {'uri': uri}).query;
    final json = await _requestList(
      'GET',
      '/sessions/$sessionId/resources/list?$query',
    );
    return json
        .whereType<Map>()
        .map((item) => MikuResourceEntry.fromJson(item.cast<String, Object?>()))
        .toList();
  }

  ResourcePreview _resourcePreviewFromJson(
    Map<String, Object?> json,
    String uri,
  ) {
    return ResourcePreview(
      uri: json['uri'] as String? ?? uri,
      kind: json['kind'] as String? ?? '',
      mime: json['mime'] as String? ?? '',
      title: json['title'] as String?,
      sizeBytes: json['size_bytes'] as int? ?? json['sizeBytes'] as int? ?? 0,
      preview: json['preview'] as String? ?? '',
      content: json['content'] as String? ?? '',
      selector: _nullableString(json['selector']),
      hasMore: json['has_more'] as bool? ?? json['hasMore'] as bool? ?? false,
    );
  }

  @override
  Future<int> assignSessionToProject(String projectId, String sessionId) async {
    final json = await _request(
      'POST',
      '/projects/$projectId/sessions/$sessionId',
      body: const {},
    );
    return (json['assigned'] as num?)?.toInt() ?? 0;
  }

  Future<Map<String, Object?>> _request(
    String method,
    String path, {
    Map<String, Object?>? body,
  }) async {
    final decoded = await _requestJson(method, path, body: body);
    if (decoded is! Map) {
      throw const FormatException('server returned a non-object JSON response');
    }
    return decoded.cast<String, Object?>();
  }

  Future<List<Object?>> _requestList(String method, String path) async {
    final decoded = await _requestJson(method, path);
    if (decoded is! List) {
      throw const FormatException('server returned a non-list JSON response');
    }
    return decoded.cast<Object?>();
  }

  Future<Object?> _requestJson(
    String method,
    String path, {
    Map<String, Object?>? body,
  }) async {
    late final web.Response response;
    try {
      response = await _fetch(
        method,
        path,
        headers: {if (body != null) 'content-type': 'application/json'},
        body: body == null ? null : jsonEncode(body).toJS,
      );
    } catch (_) {
      throw const _AmbiguousWebTransportFailure();
    }
    final status = response.status;
    if (status == 0) throw const _AmbiguousWebTransportFailure();
    if (status < 200 || status >= 300) {
      throw StateError(
        'request failed: $status ${await _responseText(response)}',
      );
    }
    final text = await _responseText(response);
    if (text.isEmpty) return <String, Object?>{};
    return jsonDecode(text);
  }

  MikuSession _sessionFromJson(
    Map<String, Object?> json, {
    String? lastEventId,
  }) {
    final modeState =
        (json['mode_state'] as Map?)?.cast<String, Object?>() ??
        (json['modeState'] as Map?)?.cast<String, Object?>() ??
        const <String, Object?>{};
    return MikuSession(
      id: json['id'] as String,
      status: json['status'] as String? ?? 'active',
      mode: (json['mode'] as String?) ?? (modeState['mode'] as String?) ?? '',
      label: json['label'] as String? ?? '',
      voiceCap:
          (json['voice_cap'] as String?) ?? (json['voiceCap'] as String?) ?? '',
      defaultScope:
          (json['memory_scope'] as String?) ??
          (json['memoryScope'] as String?) ??
          (json['default_scope'] as String?) ??
          (json['defaultScope'] as String?) ??
          'global',
      activeSkills:
          ((json['activeSkills'] as List?) ??
                  (json['active_skills'] as List?) ??
                  const [])
              .map((skill) => skill.toString())
              .toList(),
      locked:
          modeState['lockSource'] != null || modeState['lock_source'] != null,
      lastEventId: lastEventId,
    );
  }

  SessionSummary _summaryFromJson(Map<String, Object?> json) {
    return SessionSummary(
      id: json['id'] as String? ?? '',
      title: json['title'] as String? ?? 'New session',
      preview: json['preview'] as String? ?? '',
      mode: json['mode'] as String? ?? '',
      label: json['label'] as String? ?? '',
      updatedAt:
          (json['updatedAt'] as String?) ??
          (json['updated_at'] as String?) ??
          '',
      status: json['status'] as String? ?? '',
      messageCount:
          (json['messageCount'] as int?) ??
          (json['message_count'] as int?) ??
          0,
      lastEventId: _nullableString(
        json['lastEventId'] ?? json['last_event_id'],
      ),
    );
  }

  SessionMessage _messageFromJson(Map<String, Object?> json) {
    return SessionMessage(
      seq: (json['seq'] as num?)?.toInt() ?? 0,
      role: json['role'] as String? ?? '',
      content: json['content'] as String? ?? '',
      createdAt:
          (json['createdAt'] as String?) ??
          (json['created_at'] as String?) ??
          '',
    );
  }

  MikuEvent _eventFromJson(Map<String, Object?> json) {
    final data =
        (json['data'] as Map?)?.cast<String, Object?>() ??
        const <String, Object?>{};
    return MikuEvent(
      type: json['type'] as String? ?? '',
      id: _nullableString(json['id']),
      data: data,
      turnId: _nullableString(json['turnId'] ?? json['turn_id']),
      createdAt: _nullableString(json['createdAt'] ?? json['created_at']),
    );
  }

  String? _nullableString(Object? value) {
    final text = value?.toString() ?? '';
    return text.isEmpty ? null : text;
  }

  void _rememberSession(MikuSession session) {
    web.window.localStorage.setItem(_sessionIdKey, session.id);
    if (session.lastEventId != null && session.lastEventId!.isNotEmpty) {
      web.window.localStorage.setItem(_lastEventIdKey, session.lastEventId!);
    } else {
      web.window.localStorage.removeItem(_lastEventIdKey);
    }
  }

  Future<web.Response> _fetch(
    String method,
    String path, {
    Map<String, String> headers = const {},
    web.BodyInit? body,
  }) {
    final requestHeaders = web.Headers();
    for (final MapEntry(:key, :value) in headers.entries) {
      requestHeaders.set(key, value);
    }
    return web.window
        .fetch(
          path.toJS,
          web.RequestInit(
            method: method,
            credentials: 'same-origin',
            headers: requestHeaders,
            body: body,
          ),
        )
        .toDart;
  }

  Future<String> _responseText(web.Response response) async =>
      (await response.text().toDart).toDart;
}
