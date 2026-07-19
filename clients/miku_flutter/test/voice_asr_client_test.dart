import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/session_client_io.dart';
import 'package:miku_flutter/session_models.dart';

void main() {
  test('voice ASR catalog is typed and rejects mismatched engine ids', () {
    final catalog = VoiceAsrEngineCatalog.fromJson({
      'engines': const [
        {
          'id': 'local',
          'kind': 'local',
          'label': 'On-device',
          'available': true,
          'maxDurationSeconds': 60,
        },
        {
          'id': 'self_hosted',
          'kind': 'remote',
          'label': 'Home remote',
          'available': true,
          'modelId': 'tea-asr-1.1-mini',
          'maxDurationSeconds': 60,
        },
      ],
    });

    expect(catalog.selfHosted?.available, isTrue);
    expect(catalog.selfHosted?.kind, VoiceAsrEngineKind.remote);
    expect(catalog.selfHosted?.modelId, 'tea-asr-1.1-mini');
    expect(
      () => VoiceAsrEngineCatalog.fromJson({
        'engines': const [
          {
            'id': 'unexpected',
            'kind': 'remote',
            'label': 'Remote',
            'available': true,
          },
        ],
      }),
      throwsFormatException,
    );
  });

  test('native client sends bounded PCM16 with exact broker headers', () async {
    SharedPreferences.setMockInitialValues({});
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    final postSeen = Completer<void>();
    final receivedBodies = <List<int>>[];
    final receivedHeaders = <String, String?>{};
    server.listen((request) async {
      if (request.method == 'GET' && request.uri.path == '/voice/asr/engines') {
        request.response
          ..headers.contentType = ContentType.json
          ..write(
            jsonEncode({
              'engines': const [
                {
                  'id': 'local',
                  'kind': 'local',
                  'label': 'On-device',
                  'available': true,
                  'maxDurationSeconds': 60,
                },
                {
                  'id': 'self_hosted',
                  'kind': 'remote',
                  'label': 'Home remote',
                  'available': true,
                  'modelId': 'tea-asr-1.1-mini',
                  'maxDurationSeconds': 60,
                },
              ],
            }),
          );
        await request.response.close();
        return;
      }
      receivedBodies.add(
        await request.fold<List<int>>(<int>[], (all, chunk) {
          all.addAll(chunk);
          return all;
        }),
      );
      for (final name in const [
        'content-type',
        'x-tm-asr-engine-id',
        'x-tm-capture-id',
        'x-tm-sample-rate',
        'x-tm-channels',
      ]) {
        receivedHeaders[name] = request.headers.value(name);
      }
      request.response
        ..headers.contentType = ContentType.json
        ..write(
          jsonEncode({
            'text': '幫我記得倒垃圾',
            'engineId': 'self_hosted',
            'modelId': 'tea-asr-1.1-mini',
          }),
        );
      await request.response.close();
      postSeen.complete();
    });

    final client = NativeMikuSessionClient(
      tokenStore: MemoryDeviceTokenStore(),
    );
    await client.setServerBaseUrl(
      'http://${server.address.address}:${server.port}',
    );

    final catalog = await client.voiceAsrEngines();
    expect(catalog.selfHosted?.available, isTrue);
    final transcript = await client.transcribeVoicePcm16(
      engineId: selfHostedVoiceAsrEngineId,
      captureId: '12345678-1234-4abc-8def-1234567890ab',
      sampleRate: voiceAsrSampleRate,
      pcm16: Uint8List.fromList([0, 0, 1, 0]),
    );
    await postSeen.future;

    expect(transcript.text, '幫我記得倒垃圾');
    expect(transcript.engineId, selfHostedVoiceAsrEngineId);
    expect(receivedBodies, [
      [0, 0, 1, 0],
    ]);
    expect(receivedHeaders['content-type'], 'application/octet-stream');
    expect(receivedHeaders['x-tm-asr-engine-id'], 'self_hosted');
    expect(
      receivedHeaders['x-tm-capture-id'],
      '12345678-1234-4abc-8def-1234567890ab',
    );
    expect(receivedHeaders['x-tm-sample-rate'], '16000');
    expect(receivedHeaders['x-tm-channels'], '1');
  });

  test('client rejects invalid PCM before opening a request', () async {
    SharedPreferences.setMockInitialValues({});
    final client = NativeMikuSessionClient(
      tokenStore: MemoryDeviceTokenStore(),
    );
    await expectLater(
      client.transcribeVoicePcm16(
        engineId: selfHostedVoiceAsrEngineId,
        captureId: '12345678-1234-4abc-8def-1234567890ab',
        sampleRate: voiceAsrSampleRate,
        pcm16: Uint8List.fromList([1]),
      ),
      throwsArgumentError,
    );
  });

  test('native voice ASR enforces its client timeout and aborts', () async {
    SharedPreferences.setMockInitialValues({});
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    final requestSeen = Completer<void>();
    server.listen((request) async {
      await request.drain<void>();
      if (!requestSeen.isCompleted) requestSeen.complete();
      // Deliberately keep the response open. The client timeout must abort it.
    });
    final client = NativeMikuSessionClient(
      tokenStore: MemoryDeviceTokenStore(),
      voiceAsrRequestTimeout: const Duration(milliseconds: 60),
    );
    await client.setServerBaseUrl(
      'http://${server.address.address}:${server.port}',
    );

    final transcription = client.transcribeVoicePcm16(
      engineId: selfHostedVoiceAsrEngineId,
      captureId: '12345678-1234-4abc-8def-1234567890ab',
      sampleRate: voiceAsrSampleRate,
      pcm16: Uint8List.fromList([0, 0, 1, 0]),
    );
    await requestSeen.future;
    await expectLater(transcription, throwsA(isA<TimeoutException>()));
  });

  test('a request whose open completes after timeout never uploads', () async {
    SharedPreferences.setMockInitialValues({});
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    var requestsReceived = 0;
    server.listen((request) async {
      requestsReceived += 1;
      await request.drain<void>();
      await request.response.close();
    });
    final delayedHttp = HttpClient();
    addTearDown(() => delayedHttp.close(force: true));
    final openReady = Completer<void>();
    final releaseOpen = Completer<void>();
    final client = NativeMikuSessionClient(
      tokenStore: MemoryDeviceTokenStore(),
      voiceAsrRequestTimeout: const Duration(milliseconds: 200),
      openVoiceAsrRequestForTesting: (method, uri) async {
        final request = await delayedHttp.openUrl(method, uri);
        if (!openReady.isCompleted) openReady.complete();
        await releaseOpen.future;
        return request;
      },
    );
    await client.setServerBaseUrl(
      'http://${server.address.address}:${server.port}',
    );

    final transcription = client.transcribeVoicePcm16(
      engineId: selfHostedVoiceAsrEngineId,
      captureId: '12345678-1234-4abc-8def-1234567890ab',
      sampleRate: voiceAsrSampleRate,
      pcm16: Uint8List.fromList([0, 0, 1, 0]),
    );
    await openReady.future;
    await expectLater(transcription, throwsA(isA<TimeoutException>()));
    releaseOpen.complete();
    await Future<void>.delayed(const Duration(milliseconds: 100));

    expect(requestsReceived, 0);
  });

  test(
    'native voice ASR aborts on cancel and permits only one active request',
    () async {
      SharedPreferences.setMockInitialValues({});
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      addTearDown(() => server.close(force: true));
      final requestSeen = Completer<void>();
      server.listen((request) async {
        await request.drain<void>();
        if (!requestSeen.isCompleted) requestSeen.complete();
      });
      final client = NativeMikuSessionClient(
        tokenStore: MemoryDeviceTokenStore(),
      );
      await client.setServerBaseUrl(
        'http://${server.address.address}:${server.port}',
      );
      Future<VoiceAsrTranscript> transcribe() => client.transcribeVoicePcm16(
        engineId: selfHostedVoiceAsrEngineId,
        captureId: '12345678-1234-4abc-8def-1234567890ab',
        sampleRate: voiceAsrSampleRate,
        pcm16: Uint8List.fromList([0, 0, 1, 0]),
      );

      final first = transcribe();
      final firstExpectation = expectLater(first, throwsStateError);
      await requestSeen.future;
      await expectLater(transcribe(), throwsStateError);
      await client.cancelVoiceAsrTranscription();
      await firstExpectation;
    },
  );

  test('native voice ASR rejects an oversized error body at 64 KiB', () async {
    SharedPreferences.setMockInitialValues({});
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    server.listen((request) async {
      await request.drain<void>();
      request.response
        ..statusCode = HttpStatus.badGateway
        ..add(List<int>.filled(65537, 0x61));
      await request.response.close();
    });
    final client = NativeMikuSessionClient(
      tokenStore: MemoryDeviceTokenStore(),
    );
    await client.setServerBaseUrl(
      'http://${server.address.address}:${server.port}',
    );

    await expectLater(
      client.transcribeVoicePcm16(
        engineId: selfHostedVoiceAsrEngineId,
        captureId: '12345678-1234-4abc-8def-1234567890ab',
        sampleRate: voiceAsrSampleRate,
        pcm16: Uint8List.fromList([0, 0, 1, 0]),
      ),
      throwsA(
        isA<FormatException>().having(
          (error) => error.message,
          'message',
          contains('64 KiB'),
        ),
      ),
    );
  });
}
