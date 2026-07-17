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
