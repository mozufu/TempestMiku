part of '../session_client_io.dart';

extension _NativeEventsClient on NativeMikuSessionClient {
  Stream<MikuEvent> _eventsImpl(String sessionId, {String? lastEventId}) {
    final controller = StreamController<MikuEvent>();
    final eventClient = HttpClient();
    var closed = false;
    controller.onCancel = () {
      closed = true;
      eventClient.close(force: true);
    };
    unawaited(
      _pumpEvents(
        controller,
        eventClient,
        () => closed || controller.isClosed,
        sessionId,
        lastEventId,
      ),
    );
    return controller.stream;
  }

  Future<void> _pumpEvents(
    StreamController<MikuEvent> controller,
    HttpClient eventClient,
    bool Function() isClosed,
    String sessionId,
    String? initialLastEventId,
  ) async {
    var resumeId = initialLastEventId ?? await _storedLastEventId();
    if (numericEventId(resumeId) == null) resumeId = null;
    final lifecycle = SessionEventLifecycle(resumeId);
    while (!isClosed() && lifecycle.shouldReconnect) {
      try {
        final baseUrl = await serverBaseUrl();
        final request = await eventClient.getUrl(
          _resolveAgainst(baseUrl, _eventsPath(sessionId, resumeId)),
        );
        request.headers
          ..set(HttpHeaders.acceptHeader, 'text/event-stream')
          ..set(HttpHeaders.cacheControlHeader, 'no-cache');
        final token = await _deviceToken(requestBaseUrl: baseUrl);
        if (token != null) {
          request.headers.set(HttpHeaders.authorizationHeader, 'Bearer $token');
        }
        if (resumeId != null && resumeId.isNotEmpty) {
          request.headers.set('Last-Event-ID', resumeId);
        }
        final response = await request.close();
        if (response.statusCode < 200 || response.statusCode >= 300) {
          throw StateError('event stream failed: ${response.statusCode}');
        }
        if (!isClosed()) {
          controller.add(
            const MikuEvent(type: 'connection', data: {'status': 'connected'}),
          );
        }
        final decoder = SessionEventSseDecoder();
        eventStream:
        await for (final chunk in response.transform(utf8.decoder)) {
          for (final event in decoder.add(chunk)) {
            if (isClosed()) break;
            if (!lifecycle.accept(event)) continue;
            final eventId = event.id!;
            if (shouldRememberEventId(event.type, event.data)) {
              resumeId = eventId;
              await _rememberLastEventId(sessionId, eventId);
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
              await _rememberLastEventId(sessionId, eventId);
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
      }
    }
    eventClient.close(force: true);
    if (lifecycle.isTerminal && !controller.isClosed) {
      await controller.close();
    }
  }

  void _rememberLastEventIdImpl(String sessionId, String lastEventId) {
    unawaited(_rememberLastEventId(sessionId, lastEventId));
  }

  String _eventsPath(String sessionId, String? lastEventId) {
    if (lastEventId == null || lastEventId.isEmpty) {
      return '/sessions/$sessionId/events';
    }
    final query = Uri(queryParameters: {'lastEventId': lastEventId}).query;
    return '/sessions/$sessionId/events?$query';
  }

  Future<String?> _storedLastEventId() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(NativeMikuSessionClient._lastEventIdKey);
  }

  Future<void> _rememberLastEventId(
    String sessionId,
    String lastEventId,
  ) async {
    final prefs = await SharedPreferences.getInstance();
    if (prefs.getString(NativeMikuSessionClient._sessionIdKey) == sessionId) {
      await prefs.setString(
        NativeMikuSessionClient._lastEventIdKey,
        lastEventId,
      );
    }
  }
}
