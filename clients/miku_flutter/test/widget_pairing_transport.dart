part of 'widget_test.dart';

void _registerPairingAndTransportTests() {
  test('client message ids are safe and unique', () {
    final first = newClientMessageId();
    final second = newClientMessageId();

    expect(first, matches(RegExp(r'^m_[a-f0-9]{32}$')));
    expect(second, isNot(first));
  });

  test(
    'ambiguous message retry keeps one client message id and is bounded',
    () async {
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
    },
  );

  test('pairing deep links parse and normalize server targets', () {
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
    expect(
      pairingTargetFromLink(
        'tempestmiku://pair?v=1&server=https%3A%2F%2Fmiku.tailnet.test%3A8787&code=$code',
      ).serverBaseUrl,
      'https://miku.tailnet.test:8787',
    );
    expect(
      () => pairingTargetFromLink('tempestmiku://pair'),
      throwsFormatException,
    );
    expect(
      () => pairingTargetFromLink(
        'tempestmiku://pair?v=1&server=ftp%3A%2F%2Fexample.test&code=$code',
      ),
      throwsFormatException,
    );
    expect(
      () => pairingTargetFromLink(
        'https://example.test/pair?server=http%3A%2F%2Fhost&code=$code',
      ),
      throwsFormatException,
    );
    expect(
      () => pairingTargetFromLink(
        'tempestmiku://pair?v=1&server=https%3A%2F%2Fexample.test&code=short',
      ),
      throwsFormatException,
    );
    expect(
      () => pairingTargetFromLink(
        'tempestmiku://pair?v=2&server=https%3A%2F%2Fexample.test&code=$code',
      ),
      throwsFormatException,
    );
  });

  test('server targets reject embedded credentials and non-origin URLs', () {
    expect(
      () => normalizeMikuServerBaseUrl(
        'https://owner:secret@example.test',
        requireHttps: true,
      ),
      throwsFormatException,
    );
    for (final value in [
      'https://example.test/api',
      'https://example.test?token=secret',
      'https://example.test/#fragment',
    ]) {
      expect(
        () => normalizeMikuServerBaseUrl(value, requireHttps: true),
        throwsFormatException,
      );
    }
  });

  test('release server policy requires HTTPS even for loopback', () {
    expect(
      () => normalizeMikuServerBaseUrl(
        'http://127.0.0.1:8787',
        requireHttps: true,
      ),
      throwsFormatException,
    );
    expect(
      () => normalizeMikuServerBaseUrl(
        'http://localhost:8787',
        requireHttps: true,
      ),
      throwsFormatException,
    );
    expect(
      normalizeMikuServerBaseUrl(
        'https://miku.example.test',
        requireHttps: true,
      ),
      'https://miku.example.test',
    );
  });

  testWidgets('pair confirmation exposes exact authority and device name', (
    tester,
  ) async {
    const code =
        '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
    final target = pairingTargetFromLink(
      'tempestmiku://pair?v=1&server=https%3A%2F%2Fmiku.example.test%3A9443&code=$code',
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: PairingAuthorityDetails(
            target: target,
            deviceName: 'TempestMiku android',
          ),
        ),
      ),
    );

    expect(find.text('Origin: https://miku.example.test:9443'), findsOneWidget);
    expect(find.text('Scheme: HTTPS'), findsOneWidget);
    expect(find.text('Host: miku.example.test'), findsOneWidget);
    expect(find.text('Effective port: 9443'), findsOneWidget);
    expect(find.text('Device name: TempestMiku android'), findsOneWidget);
  });

  test(
    'native server target persistence clears the session cursor on change',
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

  test(
    'origin-bound credentials fail closed across interrupted switches',
    () async {
      SharedPreferences.setMockInitialValues({
        'tempestmiku.serverBaseUrl': 'https://new.example',
      });
      final staleStore =
          io_client.MemoryDeviceTokenStore()
            ..credential = const io_client.DeviceCredential(
              serverBaseUrl: 'https://old.example',
              token: 'tmk_dev_old',
            );
      final staleClient = io_client.NativeMikuSessionClient(
        tokenStore: staleStore,
      );
      expect(await staleClient.deviceTokenForTesting(), isNull);

      SharedPreferences.setMockInitialValues({
        'tempestmiku.serverBaseUrl': 'https://old.example',
      });
      final futureStore =
          io_client.MemoryDeviceTokenStore()
            ..credential = const io_client.DeviceCredential(
              serverBaseUrl: 'https://new.example',
              token: 'tmk_dev_new',
            );
      final futureClient = io_client.NativeMikuSessionClient(
        tokenStore: futureStore,
      );
      expect(await futureClient.deviceTokenForTesting(), isNull);
    },
  );

  test('a failed credential clear never publishes the new server', () async {
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

  test('session SSE decoder validates envelopes and deduplicates sequence', () {
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
    expect(events.single.turnId, isNull);
    expect(events.single.createdAt, '2026-07-10T00:00:00Z');
    expect(events.single.data['delta'], 'mi');

    final deduplicator = NumericEventDeduplicator('6');
    expect(deduplicator.accept(events.single), isTrue);
    expect(deduplicator.accept(events.single), isFalse);
    expect(
      () => SessionEventSseDecoder().add(
        'id: nope\nevent: session_event\n'
        'data: {"type":"text","turnId":null,"payload":{},'
        '"createdAt":"2026-07-10T00:00:00Z"}\n\n',
      ),
      throwsFormatException,
    );
  });

  test('session event lifecycle fences reconnect and post-end rows', () {
    final lifecycle = SessionEventLifecycle('6');
    const text = MikuEvent(type: 'text', id: '7', data: {'delta': 'miku'});
    const ended = MikuEvent(
      type: 'session_end',
      id: '8',
      data: {'status': 'ended'},
    );
    const corruptPostEnd = MikuEvent(
      type: 'text',
      id: '9',
      data: {'delta': 'must not render'},
    );

    expect(lifecycle.accept(text), isTrue);
    expect(lifecycle.shouldReconnect, isTrue);
    expect(lifecycle.accept(ended), isTrue);
    expect(lifecycle.isTerminal, isTrue);
    expect(lifecycle.shouldReconnect, isFalse);
    expect(lifecycle.accept(corruptPostEnd), isFalse);
  });

  test('does not advance the persisted cursor past unresolved gates', () {
    expect(shouldRememberEventId('approval', const {}), isFalse);
    expect(
      shouldRememberEventId('write_proposal', const {
        'kind': 'memory',
        'status': 'pending',
      }),
      isFalse,
    );
    expect(
      shouldRememberEventId('write_proposal', const {
        'kind': 'memory',
        'status': 'approved',
      }),
      isTrue,
    );
    expect(shouldRememberEventId('drive_put', const {}), isTrue);
  });
}

void _registerAsyncTransportTests() {
  testWidgets(
    'failed send preserves draft and reuses its client message id on retry',
    (WidgetTester tester) async {
      final client = _FailOnceMikuClient();
      await tester.pumpWidget(MikuApp(client: client));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));

      const draft = 'retry this exact message';
      await tester.enterText(find.byType(EditableText), draft);
      await tester.pump();
      await tester.tap(find.byIcon(Icons.send));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));

      expect(find.textContaining('Message not sent:'), findsOneWidget);
      expect(
        find.byWidgetPredicate(
          (widget) =>
              widget is Semantics && widget.properties.liveRegion == true,
        ),
        findsOneWidget,
      );
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller?.text,
        draft,
      );
      expect(client.attemptedClientMessageIds, hasLength(1));

      await tester.tap(find.byIcon(Icons.send));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      expect(client.attemptedClientMessageIds, hasLength(2));
      expect(
        client.attemptedClientMessageIds[1],
        client.attemptedClientMessageIds[0],
      );
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller?.text,
        isEmpty,
      );
      expect(find.textContaining('Message not sent:'), findsNothing);
      expect(find.text('Miku heard: $draft'), findsOneWidget);
    },
  );

  testWidgets(
    'late send completion from session A cannot clear or mutate session B',
    (WidgetTester tester) async {
      final client = _ControlledSendMikuClient();
      await tester.pumpWidget(MikuApp(client: client));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));

      await tester.enterText(find.byType(EditableText), 'slow message from A');
      await tester.pump();
      await tester.tap(find.byIcon(Icons.send));
      await tester.pump();
      expect(client.slowSendStarted, isTrue);

      await _selectDestination(tester, 'Sessions');
      await _startNewSessionFromDrawer(tester);
      for (var i = 0; i < 20 && client.eventResumeIds.length < 2; i++) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      expect(client.eventResumeIds, hasLength(2));
      expect(find.text('Miku is here'), findsOneWidget);

      const sessionBDraft = 'draft that belongs to B';
      await tester.enterText(find.byType(EditableText), sessionBDraft);
      await tester.pump();

      client.completeSlowSend();
      for (var i = 0; i < 20 && !client.slowSendCompleted; i++) {
        await tester.pump(const Duration(milliseconds: 50));
      }

      expect(client.slowSendCompleted, isTrue);
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller?.text,
        sessionBDraft,
      );
      expect(find.text('slow message from A'), findsNothing);
      expect(
        find.textContaining('Miku heard: slow message from A'),
        findsNothing,
      );

      await tester.tap(find.byIcon(Icons.send));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(find.text('Miku heard: $sessionBDraft'), findsOneWidget);
    },
  );

  testWidgets(
    'slow initial connect cannot overwrite an explicit newer session',
    (WidgetTester tester) async {
      final client = _SlowInitialConnectMikuClient();
      await tester.pumpWidget(MikuApp(client: client));
      await tester.pump();
      for (var i = 0; i < 20 && !client.initialConnectRequested; i++) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      expect(client.initialConnectRequested, isTrue);

      await _selectDestination(tester, 'Sessions');
      await _startNewSessionFromDrawer(tester);
      for (var i = 0; i < 20 && client.eventResumeIds.isEmpty; i++) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      expect(client.explicitCreateRequests, 1);
      expect(client.eventResumeIds, hasLength(1));

      const newerSessionMessage = 'this belongs to the newer session';
      await tester.enterText(find.byType(EditableText), newerSessionMessage);
      await tester.pump();
      await tester.tap(find.byIcon(Icons.send));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(find.text('Miku heard: $newerSessionMessage'), findsOneWidget);

      await client.completeInitialConnect();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      expect(client.eventResumeIds, hasLength(1));
      expect(find.text(newerSessionMessage), findsOneWidget);
      expect(find.text('Miku heard: $newerSessionMessage'), findsOneWidget);
      expect(find.text('Miku is here'), findsNothing);
      expect(await client.listSessions(), hasLength(2));
    },
  );
}
