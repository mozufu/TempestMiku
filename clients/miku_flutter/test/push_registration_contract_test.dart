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
      request.response.statusCode = HttpStatus.noContent;
      await request.response.close();
      await registrationFuture;

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

      await server.close(force: true);
    },
  );
}
