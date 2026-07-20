import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/miku_api.dart';
import 'package:miku_flutter/session_client_io.dart' as io_client;

void main() {
  test(
    'project overview retains typed planning items and resource pointers',
    () {
      final overview = ProjectOverview.fromJson({
        'projectId': 'tempestmiku',
        'projectUri': 'project://tempestmiku',
        'status': 'UI wiring in progress',
        'openLoops': const [
          {
            'id': 'loop-1',
            'kind': 'open_loop',
            'text': 'Connect Settings',
            'targetUri': 'project://tempestmiku/open-loops/loop-1',
          },
        ],
        'decisions': const [
          {
            'id': 'decision-1',
            'kind': 'decision',
            'text': 'Keep chat first',
            'targetUri': 'project://tempestmiku/decisions/decision-1',
          },
        ],
        'nextActions': const [
          {
            'id': 'next-1',
            'kind': 'next_action',
            'text': 'Run Flutter gates',
            'targetUri': 'project://tempestmiku/next-actions/next-1',
          },
        ],
        'resources': const [
          {
            'id': 'resource-1',
            'kind': 'artifact',
            'text': 'artifact://proof',
            'targetUri': 'project://tempestmiku/artifacts/proof',
            'sourceUri': 'artifact://proof',
          },
        ],
      });

      expect(overview.projectId, 'tempestmiku');
      expect(overview.openLoops.single.text, 'Connect Settings');
      expect(overview.decisions.single.kind, 'decision');
      expect(overview.nextActions, ['Run Flutter gates']);
      expect(overview.resources.single.sourceUri, 'artifact://proof');
    },
  );

  test('settings models retain device revocation and runtime queue state', () {
    final device = AuthDevice.fromJson({
      'id': 'device-1',
      'name': 'Phone',
      'platform': 'android',
      'createdAt': '2026-07-18T10:00:00Z',
      'lastSeenAt': '2026-07-20T10:00:00Z',
      'revokedAt': null,
    });
    final diagnostics = ServerDiagnostics.fromJson({
      'runtime': const {
        'role': 'all',
        'postgres': true,
        'migrationsApplied': true,
        'workersEnabled': true,
        'shuttingDown': false,
        'heartbeatFailures': 2,
        'linkHydrationFailures': 1,
      },
      'queues': const {
        'turn': {'depth': 3},
        'dream': {'depth': 2},
        'scheduler': {'depth': 1},
        'approvalEffects': {'depth': 4},
        'push': {'depth': 5},
      },
      'pendingApprovals': 6,
      'leaseReclaims': 7,
    }, baseUrl: 'https://miku.example');

    expect(device.isActive, isTrue);
    expect(diagnostics.operational, isTrue);
    expect(diagnostics.turnQueueDepth, 3);
    expect(diagnostics.approvalEffectQueueDepth, 4);
    expect(diagnostics.pushQueueDepth, 5);
    expect(diagnostics.pendingApprovals, 6);
    expect(diagnostics.heartbeatFailures, 2);
  });

  test('readiness and durable turn models retain non-ready state', () {
    final readiness = ServerReadiness.fromJson({
      'status': 'not_ready',
      'runtime': const {
        'role': 'all',
        'postgres': true,
        'migrationsApplied': true,
        'workersEnabled': true,
        'shuttingDown': false,
        'memoryReadiness': {
          'schema': {
            'corrupt': {'reason': 'missing memory_records'},
          },
          'pgvector': 'disabled',
          'embeddings': 'disabled',
        },
      },
      'selfEvolution': const {'tier': 'conservative'},
    });
    final receipt = TurnReceipt.fromJson(const {
      'turnId': 'turn-1',
      'clientMessageId': 'message-1',
      'status': 'queued',
    });
    final turn = SessionTurn.fromJson(const {
      'id': 'turn-1',
      'sessionId': 'session-1',
      'clientMessageId': 'message-1',
      'content': 'hello',
      'contentHash': 'sha256:example',
      'status': 'failed',
      'createdAt': '2026-07-20T10:00:00Z',
      'updatedAt': '2026-07-20T10:00:01Z',
      'startedAt': '2026-07-20T10:00:00Z',
      'completedAt': '2026-07-20T10:00:01Z',
      'error': 'worker stopped',
    });

    expect(readiness.ready, isFalse);
    expect(readiness.memory!.schema.status, 'corrupt');
    expect(readiness.detail, contains('missing memory_records'));
    expect(receipt.isTerminal, isFalse);
    expect(turn.isTerminal, isTrue);
    expect(turn.error, 'worker stopped');
  });

  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('client message ids are safe and unique', () {
    final first = newClientMessageId();
    final second = newClientMessageId();

    expect(first, matches(RegExp(r'^m_[a-f0-9]{32}$')));
    expect(second, isNot(first));
  });

  test(
    'project catalog and resource listing models parse camelCase wire data',
    () {
      final project = ProjectCatalogEntry.fromJson({
        'id': 'tempestmiku',
        'title': 'TempestMiku',
        'status': 'active',
        'memoryScope': 'project:tempestmiku',
        'projectUri': 'project://tempestmiku',
        'linkedFoldersUri': 'project://tempestmiku/linked-folders',
        'linkedFolderUris': [
          'project://tempestmiku/linked-folders/tempestmiku/',
        ],
      });
      expect(project.id, 'tempestmiku');
      expect(project.title, 'TempestMiku');
      expect(project.status, 'active');
      expect(project.memoryScope, 'project:tempestmiku');
      expect(project.hasLinkedFolder, isTrue);
      expect(
        project.rootUri,
        'project://tempestmiku/linked-folders/tempestmiku/',
      );

      final folderless = ProjectCatalogEntry.fromJson({
        'id': 'planning',
        'title': 'Planning',
        'status': 'active',
        'memoryScope': 'project:planning',
        'projectUri': 'project://planning',
        'linkedFoldersUri': 'project://planning/linked-folders',
      });
      expect(folderless.hasLinkedFolder, isFalse);
      expect(folderless.rootUri, isEmpty);

      final directory = MikuResourceEntry.fromJson({
        'uri': 'project://tempestmiku/linked-folders/tempestmiku/docs/',
        'name': 'docs',
        'kind': 'dir',
        'sizeBytes': 128,
        'modifiedAt': '2026-07-20T00:00:00Z',
      });
      expect(directory.isDirectory, isTrue);
      expect(directory.isFile, isFalse);
      expect(directory.sizeBytes, 128);
    },
  );

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
              deviceId: '00000000-0000-4000-8000-000000000099',
            );
      final client = io_client.NativeMikuSessionClient(tokenStore: tokenStore);
      expect(
        await (client as CurrentAuthDeviceClient).currentAuthDeviceId(),
        '00000000-0000-4000-8000-000000000099',
      );
      await client.setServerBaseUrl('new.example:8787/');

      final prefs = await SharedPreferences.getInstance();
      expect(
        prefs.getString('tempestmiku.serverBaseUrl'),
        'http://new.example:8787',
      );
      expect(prefs.getString('tempestmiku.sessionId'), isNull);
      expect(prefs.getString('tempestmiku.lastEventId'), isNull);
      expect(tokenStore.credential, isNull);
      expect(await client.currentAuthDeviceId(), isNull);
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
