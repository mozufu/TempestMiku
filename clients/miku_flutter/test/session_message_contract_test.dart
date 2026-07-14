import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/session_client_io.dart';
import 'package:miku_flutter/session_client_stub.dart';

void main() {
  const clientMessageId = 'm_0123456789abcdef0123456789abcdef';

  test(
    'native retry sends the caller-owned client message id unchanged',
    () async {
      SharedPreferences.setMockInitialValues({});
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      addTearDown(() => server.close(force: true));

      final bodies = <Map<String, Object?>>[];
      final secondAttempt = Completer<void>();
      var attempt = 0;
      server.listen((request) async {
        final body = await utf8.decoder.bind(request).join();
        bodies.add((jsonDecode(body) as Map).cast<String, Object?>());
        attempt += 1;
        if (attempt == 1) {
          final socket = await request.response.detachSocket();
          socket.destroy();
          return;
        }
        request.response
          ..statusCode = HttpStatus.ok
          ..write('{}');
        await request.response.close();
        secondAttempt.complete();
      });

      final client = NativeMikuSessionClient(
        tokenStore: MemoryDeviceTokenStore(),
      );
      await client.setServerBaseUrl(
        'http://${server.address.address}:${server.port}',
      );

      await client.sendMessage(
        'session-1',
        'hello',
        clientMessageId: clientMessageId,
      );
      await secondAttempt.future;

      expect(bodies, hasLength(2));
      expect(
        bodies.map((body) => body['clientMessageId']),
        everyElement(clientMessageId),
      );
      expect(bodies.map((body) => body['content']), everyElement('hello'));
    },
  );

  test('scripted client deduplicates a repeated caller-owned id', () async {
    final client = ScriptedMikuClient();
    final session = await client.createSession();

    await client.sendMessage(
      session.id,
      'hello',
      clientMessageId: clientMessageId,
    );
    await client.sendMessage(
      session.id,
      'hello',
      clientMessageId: clientMessageId,
    );

    final loaded = await client.loadSession(session.id);
    expect(client.sentClientMessageIds, [clientMessageId, clientMessageId]);
    expect(loaded.messages.map((message) => message.role), [
      'user',
      'assistant',
    ]);
  });
}
