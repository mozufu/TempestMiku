part of '../session_client_io.dart';

extension _NativeSettingsClient on NativeMikuSessionClient {
  Future<ServerReadiness> _serverReadinessImpl() async {
    final baseUrl = await serverBaseUrl();
    final request = await _http.openUrl(
      'GET',
      _resolveAgainst(baseUrl, '/ready'),
    );
    request.headers.set(HttpHeaders.acceptHeader, 'application/json');
    final token = await _deviceToken(requestBaseUrl: baseUrl);
    if (token != null) {
      request.headers.set(HttpHeaders.authorizationHeader, 'Bearer $token');
    }
    final response = await request.close();
    final text = await response.transform(utf8.decoder).join();
    if (response.statusCode != HttpStatus.ok &&
        response.statusCode != HttpStatus.serviceUnavailable) {
      throw StateError('request failed: ${response.statusCode} $text');
    }
    if (text.isEmpty) {
      throw const FormatException('readiness returned an empty response');
    }
    final decoded = jsonDecode(text);
    if (decoded is! Map) {
      throw const FormatException(
        'readiness returned a non-object JSON response',
      );
    }
    return ServerReadiness.fromJson(decoded.cast<String, Object?>());
  }

  Future<ServerDiagnostics> _serverDiagnosticsImpl() async {
    final baseUrl = await serverBaseUrl();
    final json = await _request('GET', '/metrics');
    return ServerDiagnostics.fromJson(json, baseUrl: baseUrl);
  }

  Future<List<AuthDevice>> _authDevicesImpl() async {
    final json = await _request('GET', '/auth/devices');
    return ((json['devices'] as List?) ?? const [])
        .whereType<Map>()
        .map((item) => AuthDevice.fromJson(item.cast<String, Object?>()))
        .toList();
  }

  Future<PairingCode> _createPairingCodeImpl() async {
    final json = await _request('POST', '/auth/pairing-codes');
    return PairingCode.fromJson(json);
  }

  Future<void> _revokeAuthDeviceImpl(String deviceId) async {
    await _request('DELETE', '/auth/devices/$deviceId');
  }

  Future<void> _endSessionImpl(String sessionId) async {
    await _request('POST', '/sessions/$sessionId/end');
  }
}
