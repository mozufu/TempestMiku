part of '../session_client_io.dart';

extension _NativeSessionsClient on NativeMikuSessionClient {
  Future<ModeCatalog> _modeCatalogImpl() async {
    final json = await _request('GET', '/modes');
    return ModeCatalog.fromJson(json);
  }

  Future<MikuSession> _createOrReuseSessionImpl() async {
    final prefs = await SharedPreferences.getInstance();
    final storedId = prefs.getString(NativeMikuSessionClient._sessionIdKey);
    if (storedId != null && storedId.isNotEmpty) {
      try {
        final json = await _request('GET', '/sessions/$storedId');
        final session = _sessionFromJson(
          json,
          lastEventId: prefs.getString(NativeMikuSessionClient._lastEventIdKey),
        );
        await _rememberSession(session);
        return session;
      } catch (_) {
        await prefs.remove(NativeMikuSessionClient._sessionIdKey);
        await prefs.remove(NativeMikuSessionClient._lastEventIdKey);
      }
    }
    return createSession();
  }

  Future<MikuSession> _createSessionImpl() async {
    final json = await _request('POST', '/sessions');
    final session = _sessionFromJson(json);
    await _rememberSession(session);
    return session;
  }

  Future<List<SessionSummary>> _listSessionsImpl({int limit = 30}) async {
    final query = Uri(queryParameters: {'limit': '$limit'}).query;
    final json = await _request('GET', '/sessions?$query');
    return ((json['sessions'] as List?) ?? const [])
        .whereType<Map>()
        .map((item) => _summaryFromJson(item.cast<String, Object?>()))
        .toList();
  }

  Future<LoadedSession> _loadSessionImpl(String sessionId) async {
    final json = await _request('GET', '/sessions/$sessionId/messages');
    final lastEventId = _nullableString(
      json['lastEventId'] ?? json['last_event_id'],
    );
    final session = _sessionFromJson(json, lastEventId: lastEventId);
    await _rememberSession(session);
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

  Future<void> _sendMessageImpl(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) async {
    await sendIdempotentMessageWithRetry(
      clientMessageId: clientMessageId,
      isAmbiguousFailure:
          (error) => error is IOException || error is TimeoutException,
      send: (stableClientMessageId) async {
        await _request(
          'POST',
          '/sessions/$sessionId/messages',
          body: {'clientMessageId': stableClientMessageId, 'content': content},
        );
      },
    );
  }

  Future<void> _resolveApprovalImpl(
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

  Future<void> _lockModeImpl(String sessionId, String mode) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/lock',
      body: {'mode': mode, 'reason': 'flutter lock'},
    );
  }

  Future<void> _unlockModeImpl(String sessionId) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/unlock',
      body: {'reason': 'flutter unlock'},
    );
  }

  Future<void> _overrideModeImpl(String sessionId, String mode) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/override',
      body: {'mode': mode, 'reason': 'flutter override'},
    );
  }

  Future<void> _rememberSession(MikuSession session) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(NativeMikuSessionClient._sessionIdKey, session.id);
    if (session.lastEventId != null && session.lastEventId!.isNotEmpty) {
      await prefs.setString(
        NativeMikuSessionClient._lastEventIdKey,
        session.lastEventId!,
      );
    } else {
      await prefs.remove(NativeMikuSessionClient._lastEventIdKey);
    }
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
    );
  }

  String? _nullableString(Object? value) {
    final text = value?.toString() ?? '';
    return text.isEmpty ? null : text;
  }
}
