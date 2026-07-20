import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/session_client_io.dart';
import 'package:miku_flutter/session_client_stub.dart' as stub_client;
import 'package:miku_flutter/session_models.dart';

void main() {
  test(
    'paired device identity validates the response and binds its origin',
    () {
      final identity = PairedAuthDeviceIdentity.fromPairResponse(const {
        'device': {'id': '00000000-0000-4000-8000-000000000123'},
      }, serverBaseUrl: 'https://miku.example/');

      expect(identity.serverBaseUrl, 'https://miku.example');
      expect(identity.deviceId, '00000000-0000-4000-8000-000000000123');
      expect(identity.matchesServer('https://miku.example'), isTrue);
      expect(identity.matchesServer('https://other.example'), isFalse);
      expect(
        () => PairedAuthDeviceIdentity.fromPairResponse(const {
          'device': <String, Object?>{},
        }, serverBaseUrl: 'https://miku.example'),
        throwsFormatException,
      );
      expect(
        PairedAuthDeviceIdentity.fromStored(
          serverBaseUrl: '',
          deviceId: identity.deviceId,
        ),
        isNull,
      );
      expect(
        PairedAuthDeviceIdentity.fromStored(
          serverBaseUrl: 'https://miku.example',
          deviceId: 'device id',
        ),
        isNull,
      );
    },
  );

  test('secure credential v2 retains device id and v1 stays unknown', () {
    const paired = DeviceCredential(
      serverBaseUrl: 'https://miku.example',
      token: 'tmk_dev_paired',
      deviceId: '00000000-0000-4000-8000-000000000123',
    );
    final decodedPaired = DeviceCredential.decode(paired.encode());
    expect(decodedPaired?.serverBaseUrl, paired.serverBaseUrl);
    expect(decodedPaired?.token, paired.token);
    expect(decodedPaired?.deviceId, paired.deviceId);

    const legacy = DeviceCredential(
      serverBaseUrl: 'https://miku.example',
      token: 'tmk_dev_legacy',
    );
    expect(DeviceCredential.decode(legacy.encode())?.deviceId, isNull);
    expect(
      DeviceCredential.decode(
        jsonEncode({
          'version': 2,
          'serverBaseUrl': 'https://miku.example',
          'token': 'tmk_dev_invalid',
        }),
      ),
      isNull,
    );
  });

  test(
    'native pairing publishes origin-bound id and logout clears it',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final baseUrl = 'http://127.0.0.1:${server.port}';
      SharedPreferences.setMockInitialValues({});
      final tokenStore = MemoryDeviceTokenStore();
      final client = NativeMikuSessionClient(tokenStore: tokenStore);
      const deviceId = '00000000-0000-4000-8000-000000000123';
      final requests = StreamIterator<HttpRequest>(server);

      final pairFuture = client.pairWithCode(
        MikuPairingTarget(
          serverBaseUrl: baseUrl,
          code:
              'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
        ),
      );
      expect(await requests.moveNext(), isTrue);
      final pairRequest = requests.current;
      final pairBody =
          jsonDecode(await utf8.decoder.bind(pairRequest).join()) as Map;
      pairRequest.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.json
        ..write(
          jsonEncode({
            'token': 'tmk_dev_pair_contract',
            'device': {'id': deviceId},
          }),
        );
      await pairRequest.response.close();
      await pairFuture;

      expect(pairRequest.method, 'POST');
      expect(pairRequest.uri.path, '/auth/pair');
      expect(pairBody['deviceName'], isNotEmpty);
      expect(tokenStore.credential?.serverBaseUrl, baseUrl);
      expect(tokenStore.credential?.deviceId, deviceId);
      expect(await client.currentAuthDeviceId(), deviceId);

      final logoutFuture = client.logout();
      expect(await requests.moveNext(), isTrue);
      final logoutRequest = requests.current;
      await utf8.decoder.bind(logoutRequest).join();
      logoutRequest.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.json
        ..write('{}');
      await logoutRequest.response.close();
      await logoutFuture;

      expect(logoutRequest.method, 'POST');
      expect(logoutRequest.uri.path, '/auth/logout');
      expect(tokenStore.credential, isNull);
      expect(await client.currentAuthDeviceId(), isNull);
      await requests.cancel();
      await server.close(force: true);
    },
  );

  test('malformed pair response never replaces the old identity', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    const oldBaseUrl = 'https://old.example';
    final newBaseUrl = 'http://127.0.0.1:${server.port}';
    const oldDeviceId = '00000000-0000-4000-8000-000000000099';
    SharedPreferences.setMockInitialValues({
      'tempestmiku.serverBaseUrl': oldBaseUrl,
    });
    final tokenStore =
        MemoryDeviceTokenStore()
          ..credential = const DeviceCredential(
            serverBaseUrl: oldBaseUrl,
            token: 'tmk_dev_old',
            deviceId: oldDeviceId,
          );
    final client = NativeMikuSessionClient(tokenStore: tokenStore);

    final requestFuture = server.first;
    final pairFuture = client.pairWithCode(
      MikuPairingTarget(
        serverBaseUrl: newBaseUrl,
        code:
            'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
      ),
    );
    final request = await requestFuture;
    await utf8.decoder.bind(request).join();
    request.response
      ..statusCode = HttpStatus.ok
      ..headers.contentType = ContentType.json
      ..write(jsonEncode({'token': 'tmk_dev_new', 'device': {}}));
    await request.response.close();

    await expectLater(pairFuture, throwsFormatException);
    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getString('tempestmiku.serverBaseUrl'), oldBaseUrl);
    expect(tokenStore.credential?.token, 'tmk_dev_old');
    expect(await client.currentAuthDeviceId(), oldDeviceId);
    await server.close(force: true);
  });

  test('scripted target changes clear the origin-bound identity', () async {
    final client = stub_client.ScriptedMikuClient();
    expect(await client.currentAuthDeviceId(), 'device-current');

    await client.setServerBaseUrl('https://other.example');

    expect(await client.currentAuthDeviceId(), isNull);
    await client.pairWithCode(
      const MikuPairingTarget(
        serverBaseUrl: 'https://other.example',
        code:
            'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
      ),
    );
    expect(await client.currentAuthDeviceId(), 'device-current');
  });
}
