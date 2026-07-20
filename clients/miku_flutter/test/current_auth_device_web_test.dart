@TestOn('browser')
library;

import 'package:flutter_test/flutter_test.dart';
import 'package:web/web.dart' as web;

import 'package:miku_flutter/session_client_web.dart';
import 'package:miku_flutter/session_models.dart';

void main() {
  tearDown(() {
    web.window.localStorage.removeItem('tempestmiku.currentAuthDeviceId');
    web.window.localStorage.removeItem('tempestmiku.currentAuthDeviceOrigin');
  });

  test('ambiguous Web pair failure clears the previously paired id', () async {
    var failPairResponse = false;
    final client = WebMikuSessionClient(
      pairRequestForTesting: (_) async {
        if (failPairResponse) {
          throw const FormatException('pair response could not be decoded');
        }
        return const {
          'device': {'id': 'device-before-cookie-rotation'},
        };
      },
    );
    final target = MikuPairingTarget(
      serverBaseUrl: web.window.location.origin,
      code: 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
    );

    await client.pairWithCode(target);
    expect(await client.currentAuthDeviceId(), 'device-before-cookie-rotation');

    failPairResponse = true;
    await expectLater(client.pairWithCode(target), throwsFormatException);

    expect(await client.currentAuthDeviceId(), isNull);
  });

  test('definitive Web pair rejection keeps the existing device id', () async {
    var rejectPairing = false;
    final client = WebMikuSessionClient(
      pairRequestForTesting: (_) async {
        if (rejectPairing) throw StateError('request failed: 401');
        return const {
          'device': {'id': 'device-with-valid-cookie'},
        };
      },
    );
    final target = MikuPairingTarget(
      serverBaseUrl: web.window.location.origin,
      code: 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
    );

    await client.pairWithCode(target);
    rejectPairing = true;
    await expectLater(client.pairWithCode(target), throwsStateError);

    expect(await client.currentAuthDeviceId(), 'device-with-valid-cookie');
  });
}
