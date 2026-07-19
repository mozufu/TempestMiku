part of '../session_client_io.dart';

extension _NativeTransport on NativeMikuSessionClient {
  Future<Map<String, Object?>> _request(
    String method,
    String path, {
    Map<String, Object?>? body,
  }) async {
    final baseUrl = await serverBaseUrl();
    final request = await _http.openUrl(method, _resolveAgainst(baseUrl, path));
    request.headers.set(HttpHeaders.acceptHeader, 'application/json');
    final token = await _deviceToken(requestBaseUrl: baseUrl);
    if (token != null) {
      request.headers.set(HttpHeaders.authorizationHeader, 'Bearer $token');
    }
    if (body != null) {
      request.headers.contentType = ContentType.json;
      request.write(jsonEncode(body));
    }
    final response = await request.close();
    final text = await response.transform(utf8.decoder).join();
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw StateError('request failed: ${response.statusCode} $text');
    }
    if (text.isEmpty) return <String, Object?>{};
    return (jsonDecode(text) as Map).cast<String, Object?>();
  }

  Future<Map<String, Object?>> _binaryRequest(
    String method,
    String path, {
    required Uint8List body,
    required Map<String, String> headers,
  }) async {
    if (_voiceAsrRequestActive) {
      throw StateError('a voice ASR request is already active');
    }
    _voiceAsrRequestActive = true;
    final requestEpoch = _voiceAsrRequestEpoch;
    final done = Completer<void>();
    final cancellation = Completer<Map<String, Object?>>();
    _activeVoiceAsrDone = done;
    _activeVoiceAsrCancellation = cancellation;
    HttpClientRequest? request;
    void ensureCurrent() {
      if (requestEpoch == _voiceAsrRequestEpoch) return;
      final error = StateError('voice ASR transcription cancelled');
      request?.abort(error);
      throw error;
    }

    Future<Map<String, Object?>> operation() async {
      final baseUrl = await serverBaseUrl();
      ensureCurrent();
      final uri = _resolveAgainst(baseUrl, path);
      request = await switch (openVoiceAsrRequestForTesting) {
        final opener? => opener(method, uri),
        null => _http.openUrl(method, uri),
      };
      ensureCurrent();
      _activeVoiceAsrRequest = request;
      request!.headers
        ..set(HttpHeaders.acceptHeader, 'application/json')
        ..contentType = ContentType.binary;
      for (final entry in headers.entries) {
        request!.headers.set(entry.key, entry.value);
      }
      final token = await _deviceToken(requestBaseUrl: baseUrl);
      if (token != null) {
        request!.headers.set(HttpHeaders.authorizationHeader, 'Bearer $token');
      }
      ensureCurrent();
      request!.add(body);
      final response = await request!.close();
      ensureCurrent();
      final bytes = await _readBoundedResponse(response);
      ensureCurrent();
      final text = utf8.decode(bytes, allowMalformed: false);
      if (response.statusCode < 200 || response.statusCode >= 300) {
        throw StateError('request failed: ${response.statusCode} $text');
      }
      if (text.isEmpty) return <String, Object?>{};
      final decoded = jsonDecode(text);
      if (decoded is! Map) {
        throw const FormatException('voice ASR returned invalid JSON');
      }
      return decoded.cast<String, Object?>();
    }

    try {
      return await Future.any([operation(), cancellation.future]).timeout(
        voiceAsrRequestTimeout,
        onTimeout: () {
          final error = TimeoutException(
            'voice ASR request timed out',
            voiceAsrRequestTimeout,
          );
          if (_voiceAsrRequestEpoch == requestEpoch) {
            _voiceAsrRequestEpoch += 1;
          }
          request?.abort(error);
          throw error;
        },
      );
    } finally {
      if (identical(_activeVoiceAsrRequest, request)) {
        _activeVoiceAsrRequest = null;
      }
      _voiceAsrRequestActive = false;
      if (identical(_activeVoiceAsrDone, done)) {
        _activeVoiceAsrDone = null;
      }
      if (identical(_activeVoiceAsrCancellation, cancellation)) {
        _activeVoiceAsrCancellation = null;
      }
      if (!cancellation.isCompleted) {
        cancellation.complete(<String, Object?>{});
      }
      if (!done.isCompleted) done.complete();
    }
  }

  Future<Uint8List> _readBoundedResponse(
    HttpClientResponse response, {
    int maxBytes = 65536,
  }) async {
    final bytes = BytesBuilder(copy: false);
    final iterator = StreamIterator<List<int>>(response);
    try {
      while (await iterator.moveNext()) {
        final chunk = iterator.current;
        if (bytes.length + chunk.length > maxBytes) {
          throw const FormatException(
            'voice ASR response exceeds the 64 KiB limit',
          );
        }
        bytes.add(chunk);
      }
      return bytes.takeBytes();
    } finally {
      await iterator.cancel();
    }
  }

  Future<Map<String, Object?>> _pairRequest(
    String baseUrl,
    Map<String, Object?> body,
  ) async {
    final baseUri = Uri.parse(baseUrl.endsWith('/') ? baseUrl : '$baseUrl/');
    final request = await _http.postUrl(baseUri.resolve('auth/pair'));
    request.headers
      ..set(HttpHeaders.acceptHeader, 'application/json')
      ..contentType = ContentType.json;
    request.write(jsonEncode(body));
    final response = await request.close();
    final text = await response.transform(utf8.decoder).join();
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw StateError('pairing failed: ${response.statusCode} $text');
    }
    return (jsonDecode(text) as Map).cast<String, Object?>();
  }

  Uri _resolveAgainst(String base, String path) {
    final baseUri = Uri.parse(base.endsWith('/') ? base : '$base/');
    final relative = path.startsWith('/') ? path.substring(1) : path;
    return baseUri.resolve(relative);
  }
}
