import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/miku_api.dart';
import 'package:miku_flutter/session_client_io.dart' as io_client;

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('client message ids are safe and unique', () {
    final first = newClientMessageId();
    final second = newClientMessageId();

    expect(first, matches(RegExp(r'^m_[a-f0-9]{32}$')));
    expect(second, isNot(first));
  });

  test('ambiguous message retry keeps one id and is bounded', () async {
    const clientMessageId = 'm_0123456789abcdef0123456789abcdef';
    final attemptedIds = <String>[];

    await expectLater(
      sendIdempotentMessageWithRetry(
        clientMessageId: clientMessageId,
        retryDelay: Duration.zero,
        isAmbiguousFailure: (_) => true,
        send: (id) async {
          attemptedIds.add(id);
          throw StateError('ambiguous transport failure');
        },
      ),
      throwsStateError,
    );
    expect(attemptedIds, const [clientMessageId, clientMessageId]);
  });

  test('pairing links parse and normalize exact server origins', () {
    const code =
        '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
    final target = pairingTargetFromLink(
      'tempestmiku://pair?v=1&server=http%3A%2F%2F192.168.1.50%3A8787%2F&code=$code',
    );
    expect(target.serverBaseUrl, 'http://192.168.1.50:8787');
    expect(target.code, code);
    expect(target.origin, 'http://192.168.1.50:8787');
    expect(target.scheme, 'HTTP');
    expect(target.host, '192.168.1.50');
    expect(target.effectivePort, 8787);

    for (final invalid in [
      'tempestmiku://pair',
      'tempestmiku://pair?v=1&server=ftp%3A%2F%2Fexample.test&code=$code',
      'https://example.test/pair?server=http%3A%2F%2Fhost&code=$code',
      'tempestmiku://pair?v=1&server=https%3A%2F%2Fexample.test&code=short',
      'tempestmiku://pair?v=2&server=https%3A%2F%2Fexample.test&code=$code',
    ]) {
      expect(() => pairingTargetFromLink(invalid), throwsFormatException);
    }
  });

  test(
    'server targets reject credentials, paths, queries, and insecure release URLs',
    () {
      for (final value in [
        'https://owner:secret@example.test',
        'https://example.test/api',
        'https://example.test?token=secret',
        'https://example.test/#fragment',
        'http://127.0.0.1:8787',
        'http://localhost:8787',
      ]) {
        expect(
          () => normalizeMikuServerBaseUrl(value, requireHttps: true),
          throwsFormatException,
        );
      }
      expect(
        normalizeMikuServerBaseUrl(
          'https://miku.example.test',
          requireHttps: true,
        ),
        'https://miku.example.test',
      );
    },
  );

  test(
    'native target changes clear credentials and the event cursor',
    () async {
      SharedPreferences.setMockInitialValues({
        'tempestmiku.serverBaseUrl': 'http://old.example:8787',
        'tempestmiku.sessionId': 'old-session',
        'tempestmiku.lastEventId': '42',
      });
      final tokenStore =
          io_client.MemoryDeviceTokenStore()
            ..credential = const io_client.DeviceCredential(
              serverBaseUrl: 'http://old.example:8787',
              token: 'tmk_dev_old',
            );
      final client = io_client.NativeMikuSessionClient(tokenStore: tokenStore);
      await client.setServerBaseUrl('new.example:8787/');

      final prefs = await SharedPreferences.getInstance();
      expect(
        prefs.getString('tempestmiku.serverBaseUrl'),
        'http://new.example:8787',
      );
      expect(prefs.getString('tempestmiku.sessionId'), isNull);
      expect(prefs.getString('tempestmiku.lastEventId'), isNull);
      expect(tokenStore.credential, isNull);
    },
  );

  test('failed credential clearing never publishes a new server', () async {
    SharedPreferences.setMockInitialValues({
      'tempestmiku.serverBaseUrl': 'https://old.example',
      'tempestmiku.sessionId': 'old-session',
      'tempestmiku.lastEventId': '42',
    });
    final client = io_client.NativeMikuSessionClient(
      tokenStore: _FailingDeleteTokenStore(),
    );

    await expectLater(
      client.setServerBaseUrl('https://new.example'),
      throwsStateError,
    );
    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getString('tempestmiku.serverBaseUrl'), 'https://old.example');
    expect(prefs.getString('tempestmiku.sessionId'), 'old-session');
    expect(prefs.getString('tempestmiku.lastEventId'), '42');
  });

  test('SSE decoder validates envelopes and deduplicates numeric ids', () {
    final decoder = SessionEventSseDecoder();
    expect(decoder.add('id: 7\nevent: session_'), isEmpty);
    final events = decoder.add(
      'event\ndata: {"type":"text","turnId":null,'
      '"payload":{"delta":"mi"},'
      '"createdAt":"2026-07-10T00:00:00Z"}\n\n',
    );
    expect(events, hasLength(1));
    expect(events.single.type, 'text');
    expect(events.single.id, '7');
    expect(events.single.data['delta'], 'mi');

    final deduplicator = NumericEventDeduplicator('6');
    expect(deduplicator.accept(events.single), isTrue);
    expect(deduplicator.accept(events.single), isFalse);
  });

  test('terminal session events fence reconnects and later rows', () {
    final lifecycle = SessionEventLifecycle('6');
    const text = MikuEvent(type: 'text', id: '7', data: {'delta': 'miku'});
    const ended = MikuEvent(
      type: 'session_end',
      id: '8',
      data: {'status': 'ended'},
    );
    const postEnd = MikuEvent(
      type: 'text',
      id: '9',
      data: {'delta': 'must not render'},
    );

    expect(lifecycle.accept(text), isTrue);
    expect(lifecycle.accept(ended), isTrue);
    expect(lifecycle.shouldReconnect, isFalse);
    expect(lifecycle.accept(postEnd), isFalse);
  });

  test('unresolved approval gates do not advance the durable cursor', () {
    expect(shouldRememberEventId('approval', const {}), isFalse);
    expect(
      shouldRememberEventId('write_proposal', const {
        'kind': 'memory',
        'status': 'pending',
      }),
      isFalse,
    );
    expect(shouldRememberEventId('drive_put', const {}), isTrue);
  });
}

class _FailingDeleteTokenStore implements io_client.DeviceTokenStore {
  @override
  Future<void> delete() => Future<void>.error(StateError('simulated crash'));

  @override
  Future<io_client.DeviceCredential?> read() async =>
      const io_client.DeviceCredential(
        serverBaseUrl: 'https://old.example',
        token: 'tmk_dev_old',
      );

  @override
  Future<void> write(io_client.DeviceCredential credential) async {}
}
