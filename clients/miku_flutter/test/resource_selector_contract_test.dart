import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/session_client_io.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('resource resolve encodes and retains the exact selector', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    final requestedUris = <String>[];
    server.listen((request) async {
      requestedUris.add(request.uri.toString());
      request.response
        ..headers.contentType = ContentType.json
        ..write(
          jsonEncode({
            'uri': 'history://scripted-actor/output',
            'kind': 'text',
            'mime': 'text/plain',
            'title': 'Actor output',
            'size_bytes': 4096,
            'selector': '201-400',
            'has_more': false,
            'preview': 'line 201',
            'content': 'line 201\nline 400',
          }),
        );
      await request.response.close();
    });
    final client = NativeMikuSessionClient(
      tokenStore: MemoryDeviceTokenStore(),
    );
    await client.setServerBaseUrl(
      'http://${server.address.address}:${server.port}',
    );

    final resource = await client.resolveResource(
      'session-1',
      'history://scripted-actor/output',
      selector: '201-400',
    );

    expect(requestedUris, [
      '/sessions/session-1/resources/resolve?uri=history%3A%2F%2Fscripted-actor%2Foutput&selector=201-400',
    ]);
    expect(resource.selector, '201-400');
    expect(resource.content, 'line 201\nline 400');
    expect(resource.hasMore, isFalse);
  });
}
