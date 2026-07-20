import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/session_client_io.dart';

void main() {
  test(
    'push registration uses the authenticated server contract route',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final baseUrl = 'http://127.0.0.1:${server.port}';
      SharedPreferences.setMockInitialValues({
        'tempestmiku.serverBaseUrl': baseUrl,
      });
      final tokenStore =
          MemoryDeviceTokenStore()
            ..credential = DeviceCredential(
              serverBaseUrl: baseUrl,
              token: 'tmk_dev_push_contract_test',
            );
      final client = NativeMikuSessionClient(tokenStore: tokenStore);

      final requestFuture = server.first;
      final registrationFuture = client.registerPush(
        endpoint: 'https://push.example.test/up-test',
        p256dh: 'p256dh-test',
        auth: 'auth-test',
      );
      final request = await requestFuture;
      final body = jsonDecode(await utf8.decoder.bind(request).join()) as Map;
      request.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.json
        ..write(
          jsonEncode({
            'registration': {
              'deviceId': '00000000-0000-4000-8000-000000000001',
              'provider': 'unifiedpush',
              'createdAt': '2026-07-20T01:00:00Z',
              'updatedAt': '2026-07-20T02:00:00Z',
              'disabledAt': null,
            },
          }),
        );
      await request.response.close();
      final metadata = await registrationFuture;

      expect(request.method, 'PUT');
      expect(request.uri.path, '/auth/push-registration');
      expect(
        request.headers.value(HttpHeaders.authorizationHeader),
        'Bearer tmk_dev_push_contract_test',
      );
      expect(body['provider'], 'unifiedpush');
      expect(jsonDecode(body['registration'] as String), {
        'endpoint': 'https://push.example.test/up-test',
        'p256dh': 'p256dh-test',
        'auth': 'auth-test',
      });
      expect(metadata.deviceId, '00000000-0000-4000-8000-000000000001');
      expect(metadata.provider, 'unifiedpush');
      expect(metadata.updatedAt, '2026-07-20T02:00:00Z');
      expect(metadata.acknowledgedActive, isTrue);

      await server.close(force: true);
    },
  );

  test(
    'push registration rejects a success response without metadata',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final baseUrl = 'http://127.0.0.1:${server.port}';
      SharedPreferences.setMockInitialValues({
        'tempestmiku.serverBaseUrl': baseUrl,
      });
      final tokenStore =
          MemoryDeviceTokenStore()
            ..credential = DeviceCredential(
              serverBaseUrl: baseUrl,
              token: 'tmk_dev_push_contract_test',
            );
      final client = NativeMikuSessionClient(tokenStore: tokenStore);

      final requestFuture = server.first;
      final registrationFuture = client.registerPush(
        endpoint: 'https://push.example.test/up-test',
        p256dh: 'p256dh-test',
        auth: 'auth-test',
      );
      final request = await requestFuture;
      await utf8.decoder.bind(request).join();
      request.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.json
        ..write('{}');
      await request.response.close();

      await expectLater(registrationFuture, throwsA(isA<FormatException>()));
      await server.close(force: true);
    },
  );
}
