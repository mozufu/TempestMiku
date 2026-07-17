import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/notification_service.dart';
import 'package:miku_flutter/ratex_formula.dart';
import 'package:miku_flutter/session_client_io.dart' as io_client;
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';
import 'package:miku_flutter/session_sse.dart';

class RecordingNotificationService implements MikuNotificationService {
  final List<String> shownApprovals = [];
  final List<String> cancelledApprovals = [];
  var initialized = false;
  final actionController =
      StreamController<ApprovalNotificationAction>.broadcast(sync: true);

  @override
  Stream<ApprovalNotificationAction> get actions => actionController.stream;

  @override
  bool get isSupported => true;

  @override
  Future<void> cancelApproval(String approvalId) async {
    cancelledApprovals.add(approvalId);
  }

  @override
  Future<void> initialize() async {
    initialized = true;
  }

  @override
  Future<bool> requestPermission() async => true;

  @override
  Future<void> showApproval({
    required String sessionId,
    required String approvalId,
    required String action,
    String? expiresAt,
  }) async {
    shownApprovals.add('$sessionId:$approvalId');
  }
}

class ActionableRecordingNotificationService
    extends RecordingNotificationService
    implements ActionableNotificationService {
  final routeController = StreamController<NotificationRouteAction>.broadcast(
    sync: true,
  );
  NotificationReplyAuthority? configuredAuthority;

  @override
  Stream<NotificationRouteAction> get routes => routeController.stream;

  @override
  Future<void> configureReplyAuthority({
    String? serverBaseUrl,
    String? deviceToken,
  }) async {
    configuredAuthority =
        serverBaseUrl == null || deviceToken == null
            ? null
            : NotificationReplyAuthority(
              serverBaseUrl: serverBaseUrl,
              deviceToken: deviceToken,
            );
  }
}

Future<void> _openSettings(WidgetTester tester) async {
  await tester.tap(find.byTooltip('Settings').first);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
}

Future<void> _selectDestination(WidgetTester tester, String label) async {
  final navigation =
      find.byType(NavigationBar).evaluate().isNotEmpty
          ? find.byType(NavigationBar)
          : find.byType(NavigationRail);
  final destination = find.descendant(
    of: navigation,
    matching: find.text(label),
  );
  expect(destination, findsOneWidget);
  await tester.tap(destination);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 100));
}

Future<void> _scrollChatUntilVisible(WidgetTester tester, Finder target) async {
  await tester.scrollUntilVisible(
    target,
    260,
    scrollable: find.byType(Scrollable).first,
    maxScrolls: 20,
  );
  await tester.pump();
}

Future<void> _openActivitySheet(WidgetTester tester, {int round = 1}) async {
  final card = find.byKey(ValueKey('agent-activity:$round'));
  await _scrollChatUntilVisible(tester, card);
  await tester.tap(card);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
}

Future<void> _popRoute(WidgetTester tester) async {
  await tester.binding.handlePopRoute();
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
}

Future<void> _closeActivitySheet(WidgetTester tester) async {
  await _popRoute(tester);
}

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('approval notification policy only alerts outside the visible app', () {
    expect(shouldNotifyApproval(AppLifecycleState.resumed), isFalse);
    expect(shouldNotifyApproval(AppLifecycleState.inactive), isTrue);
    expect(shouldNotifyApproval(AppLifecycleState.hidden), isTrue);
    expect(shouldNotifyApproval(AppLifecycleState.paused), isTrue);
    expect(shouldNotifyApproval(AppLifecycleState.detached), isTrue);
  });

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

  test('parses server-owned evolution review apply authority', () {
    final proposal = EvolutionReviewProposal.fromEvent(const {
      'kind': 'evolution_review',
      'proposalId': 'proposal-1',
      'target': {'kind': 'mode', 'modeId': 'serious_engineer'},
      'status': 'approved',
      'preview': 'Review verification guidance.',
      'uri': 'memory://review-proposals/proposal-1',
      'applyEnabled': true,
    });
    expect(proposal, isNotNull);
    expect(proposal!.targetKind, 'mode');
    expect(proposal.targetId, 'serious_engineer');
    expect(proposal.status, 'approved');
    expect(proposal.applyEnabled, isTrue);
    expect(proposal.resourceUri, startsWith('memory://review-proposals/'));
  });

  testWidgets('renders server-owned moderate review lifecycle and approval', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    const sessionId = 'scripted-0';
    client.emitEvent(
      sessionId,
      const MikuEvent(
        type: 'write_proposal',
        id: 'review-1',
        data: {
          'kind': 'evolution_review',
          'proposalId': 'proposal-review',
          'target': {'kind': 'mode', 'modeId': 'serious_engineer'},
          'status': 'pending',
          'preview': 'Prefer replayable verification evidence.',
          'uri': 'memory://review-proposals/proposal-review',
          'applyEnabled': true,
        },
      ),
    );
    client.seedPendingApproval(
      sessionId,
      approvalId: 'approval-review',
      backend: 'evolution-review',
      action: 'review mode addendum serious_engineer',
      scope: const {
        'kind': 'evolution_review',
        'proposalId': 'proposal-review',
        'preview': 'Prefer replayable verification evidence.',
        'uri': 'memory://review-proposals/proposal-review',
        'applyEnabled': true,
      },
    );
    client.emitEvent(
      sessionId,
      const MikuEvent(
        type: 'approval',
        id: 'review-2',
        data: {
          'approvalId': 'approval-review',
          'backend': 'evolution-review',
          'action': 'review mode addendum serious_engineer',
          'scope': {
            'kind': 'evolution_review',
            'proposalId': 'proposal-review',
            'preview': 'Prefer replayable verification evidence.',
            'uri': 'memory://review-proposals/proposal-review',
            'applyEnabled': true,
          },
          'options': [
            {
              'optionId': 'allow',
              'name': 'Apply mode addendum',
              'kind': 'allow_once',
            },
            {
              'optionId': 'reject',
              'name': 'Reject proposal',
              'kind': 'reject_once',
            },
          ],
          'timeoutMs': 60000,
        },
      ),
    );
    await tester.pump();

    await _openActivitySheet(tester);
    expect(
      find.textContaining('mode addendum · serious_engineer · pending'),
      findsWidgets,
    );
    expect(find.textContaining('Apply enabled'), findsOneWidget);
    await _closeActivitySheet(tester);

    final card = find.byKey(
      const ValueKey('approval:review mode addendum serious_engineer'),
    );
    await _scrollChatUntilVisible(tester, card);
    expect(card, findsOneWidget);
    await tester.ensureVisible(card);
    await tester.tap(card);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.textContaining('applyEnabled: true'), findsOneWidget);
    expect(find.text('Apply mode addendum'), findsOneWidget);
    await tester.ensureVisible(find.text('Apply mode addendum'));
    await tester.tap(find.text('Apply mode addendum'));
    await tester.pump();
    expect(client.resolvedApprovals, contains('approval-review:approve'));
  });

  testWidgets('compact mobile chrome stays readable at 390px', (
    WidgetTester tester,
  ) async {
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('Miku'), findsOneWidget);
    expect(find.text('TempestMiku'), findsNothing);
    expect(find.text('Miku is here'), findsOneWidget);
    expect(find.text('Miku 在這裡'), findsNothing);
    expect(find.byType(NavigationBar), findsOneWidget);
    expect(find.byType(NavigationRail), findsNothing);
    expect(find.text('Chat'), findsOneWidget);
    expect(find.text('Sessions'), findsOneWidget);
    expect(find.text('Drive'), findsOneWidget);
    expect(find.byIcon(Icons.tune_rounded), findsOneWidget);
    expect(find.text('Personal'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('desktop shell exposes sessions, chat, and context panes', (
    WidgetTester tester,
  ) async {
    tester.view.physicalSize = const Size(1440, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.byType(NavigationRail), findsOneWidget);
    expect(find.byType(NavigationBar), findsNothing);
    expect(find.text('Sessions'), findsNWidgets(2));
    expect(find.text('Context'), findsOneWidget);
    expect(find.text('Project status'), findsOneWidget);
    expect(find.byType(TextField), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('session_end renders a terminal session and disables sending', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'unsent draft');
    await tester.pump();
    client.emitEvent(
      'scripted-0',
      const MikuEvent(type: 'session_end', id: '99', data: {'status': 'ended'}),
    );
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('Ended'), findsWidgets);
    final composer = tester.widget<TextField>(find.byType(TextField));
    expect(composer.enabled, isFalse);
    expect(find.byTooltip('Session ended'), findsOneWidget);
    expect(client.rememberedLastEventIds['scripted-0'], '99');
  });

  testWidgets('primary chat controls expose selected-language semantics', (
    WidgetTester tester,
  ) async {
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.byType(NavigationBar), findsOneWidget);
    expect(find.text('Chat'), findsOneWidget);
    expect(find.text('Sessions'), findsOneWidget);
    expect(find.text('Drive'), findsOneWidget);
    expect(find.byTooltip('Settings'), findsOneWidget);
    expect(find.bySemanticsLabel('Send message'), findsOneWidget);

    await tester.enterText(find.byType(EditableText), 'code artifact://0');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.byKey(const ValueKey('resource:artifact://0')), findsOneWidget);
  });

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
      await tester.tap(find.bySemanticsLabel('Create new session'));
      await tester.pump();
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
      await tester.tap(find.bySemanticsLabel('Create new session'));
      await tester.pump();
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

  testWidgets(
    'background approvals notify and resolved approvals clear alerts',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      final session = await client.createSession();
      final notifications = RecordingNotificationService();

      await tester.pumpWidget(
        MikuApp(client: client, notifications: notifications),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      expect(notifications.initialized, isTrue);

      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      await tester.pump();
      client.emitEvent(
        session.id,
        const MikuEvent(
          type: 'approval',
          data: {
            'approvalId': 'approval-background',
            'action': 'proc.run cargo clean',
            'scope': {},
            'options': [],
          },
        ),
      );
      await tester.pump();

      expect(notifications.shownApprovals, const [
        'scripted-0:approval-background',
      ]);

      client.emitEvent(
        session.id,
        const MikuEvent(
          type: 'approval_resolved',
          data: {'approvalId': 'approval-background'},
        ),
      );
      await tester.pump();

      expect(notifications.cancelledApprovals, const ['approval-background']);
    },
  );

  testWidgets(
    'notification action loads the target session and approves once',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      final session = await client.createSession();
      client.seedPendingApproval(
        session.id,
        approvalId: 'approval-notification-action',
        action: 'proc.run cargo test',
      );
      final notifications = RecordingNotificationService();
      addTearDown(notifications.actionController.close);

      await tester.pumpWidget(
        MikuApp(client: client, notifications: notifications),
      );
      await tester.pump();
      for (var i = 0; i < 20 && client.driveFeedRequests == 0; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(client.driveFeedRequests, greaterThan(0));
      expect(notifications.actionController.hasListener, isTrue);

      notifications.actionController.add(
        ApprovalNotificationAction(
          sessionId: session.id,
          approvalId: 'approval-notification-action',
          decision: 'approve',
          requiresConfirmation: false,
        ),
      );
      await tester.pump();
      for (var i = 0; i < 10 && client.resolvedApprovals.isEmpty; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }

      expect(
        client.resolvedApprovals,
        contains('approval-notification-action:approve'),
      );
      expect(
        notifications.cancelledApprovals,
        contains('approval-notification-action'),
      );
    },
  );

  testWidgets(
    'notification route restores the exact session without replaying a message',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      final target = await client.createSession();
      client.seedPendingApproval(
        target.id,
        approvalId: 'approval-route-target',
        action: 'proc.run cargo test',
      );
      await client.createSession();
      final notifications = ActionableRecordingNotificationService();
      addTearDown(notifications.actionController.close);
      addTearDown(notifications.routeController.close);

      await tester.pumpWidget(
        MikuApp(client: client, notifications: notifications),
      );
      await tester.pump();
      for (var i = 0; i < 20 && client.driveFeedRequests == 0; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }

      notifications.routeController.add(
        NotificationRouteAction(sessionId: target.id, kind: 'session_ready'),
      );
      for (var i = 0; i < 20; i++) {
        await tester.pump(const Duration(milliseconds: 100));
        if (find
            .text('Pending approval · proc.run cargo test')
            .evaluate()
            .isNotEmpty) {
          break;
        }
      }

      expect(
        find.text('Pending approval · proc.run cargo test'),
        findsOneWidget,
      );
      expect(client.sentClientMessageIds, isEmpty);
    },
  );

  testWidgets('stale notification action syncs and reports expiry', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await client.createSession();
    final notifications = RecordingNotificationService();
    addTearDown(notifications.actionController.close);

    await tester.pumpWidget(
      MikuApp(client: client, notifications: notifications),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    notifications.actionController.add(
      ApprovalNotificationAction(
        sessionId: session.id,
        approvalId: 'approval-already-expired',
        decision: 'deny',
        requiresConfirmation: false,
      ),
    );
    await tester.pump();
    for (var i = 0; i < 10 && notifications.cancelledApprovals.isEmpty; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    await tester.pump();

    expect(client.resolvedApprovals, isEmpty);
    expect(
      notifications.cancelledApprovals,
      contains('approval-already-expired'),
    );
    expect(
      find.text('This approval was already resolved or has expired.'),
      findsOneWidget,
    );
  });

  testWidgets(
    'legacy Android notification action requires in-app confirmation',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      final session = await client.createSession();
      client.seedPendingApproval(
        session.id,
        approvalId: 'approval-legacy-confirm',
        action: 'drive.put inbox/report.md',
        backend: 'drive',
      );
      final notifications = RecordingNotificationService();
      addTearDown(notifications.actionController.close);

      await tester.pumpWidget(
        MikuApp(client: client, notifications: notifications),
      );
      await tester.pump();
      for (var i = 0; i < 20 && client.driveFeedRequests == 0; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      notifications.actionController.add(
        ApprovalNotificationAction(
          sessionId: session.id,
          approvalId: 'approval-legacy-confirm',
          decision: 'deny',
          requiresConfirmation: true,
        ),
      );
      await tester.pump();
      for (
        var i = 0;
        i < 10 && find.byType(AlertDialog).evaluate().isEmpty;
        i++
      ) {
        await tester.pump(const Duration(milliseconds: 100));
      }

      expect(client.resolvedApprovals, isEmpty);
      expect(find.text('drive.put inbox/report.md'), findsWidgets);
      await tester.tap(find.widgetWithText(FilledButton, 'Deny'));
      await tester.pump();
      for (var i = 0; i < 10 && client.resolvedApprovals.isEmpty; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(
        client.resolvedApprovals,
        contains('approval-legacy-confirm:deny'),
      );
    },
  );

  testWidgets('shows and resolves pending drive filing approval', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await client.createSession();
    client.seedPendingApproval(
      session.id,
      approvalId: 'approval-drive',
      action: 'drive.put inbox/approval-drop.md',
      scope: const {
        'capability': 'drive.put',
        'sourceUri': 'drop://browser/approval-drop.md',
      },
    );
    client.seedPendingApproval(
      session.id,
      approvalId: 'approval-drive-deny',
      action: 'drive.put inbox/blocked-drop.md',
      scope: const {
        'capability': 'drive.put',
        'sourceUri': 'drop://browser/blocked-drop.md',
      },
    );

    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    final approvalCard = find.byKey(
      const ValueKey('approval:drive.put inbox/approval-drop.md'),
    );
    expect(approvalCard, findsOneWidget);
    expect(
      find.text('Pending approval · drive.put inbox/approval-drop.md'),
      findsOneWidget,
    );

    await _selectDestination(tester, 'Drive');
    expect(find.text('Pending drive approvals'), findsOneWidget);
    expect(find.text('drive.put inbox/approval-drop.md'), findsOneWidget);
    expect(find.text('drive.put inbox/blocked-drop.md'), findsOneWidget);
    await _selectDestination(tester, 'Chat');

    await _scrollChatUntilVisible(tester, approvalCard);
    await tester.ensureVisible(approvalCard);
    await tester.tap(approvalCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Approval needed'), findsOneWidget);
    await tester.tap(find.text('Approve once'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, contains('approval-drive:approve'));
    expect(approvalCard, findsNothing);

    final denyCard = find.byKey(
      const ValueKey('approval:drive.put inbox/blocked-drop.md'),
    );
    expect(denyCard, findsOneWidget);
    await tester.ensureVisible(denyCard);
    await tester.tap(denyCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.ensureVisible(find.text('Deny'));
    await tester.pump();
    await tester.tap(find.text('Deny'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, contains('approval-drive-deny:deny'));
    expect(denyCard, findsNothing);
  });

  testWidgets('dogfoods drive research feed from remote control UI', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
      find.byType(EditableText),
      'research drive workspace for p5',
    );
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 150));

    expect(find.text('Drive organizer completed'), findsWidgets);
    expect(
      find.textContaining('drive://projects/tempestmiku/research'),
      findsWidgets,
    );

    await _openActivitySheet(tester);
    expect(find.text('Drive organizer completed'), findsWidgets);
    final activityResource = find.byKey(
      const ValueKey(
        'activity-resource:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    expect(activityResource, findsWidgets);
    await tester.ensureVisible(activityResource.first);
    await tester.pump();
    await tester.tap(activityResource.first);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(find.textContaining('# Scripted drive note'), findsOneWidget);

    await _popRoute(tester);
    await _closeActivitySheet(tester);

    await _selectDestination(tester, 'Drive');

    expect(client.driveFeedRequests, greaterThan(0));
    expect(find.text('Drive'), findsWidgets);
    expect(find.text('Recent documents'), findsWidgets);
    expect(find.text('P5 drive research notes'), findsOneWidget);
    expect(find.text('Organizer proposals'), findsOneWidget);
    expect(
      find.textContaining(
        'inbox/raw-research.md -> projects/tempestmiku/research',
      ),
      findsOneWidget,
    );
    expect(find.text('Virtual folders'), findsOneWidget);

    final row = find.byKey(
      const ValueKey(
        'drive-feed:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    await tester.ensureVisible(row);
    await tester.pump();
    await tester.tap(row);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(
      find.textContaining('Local citation corpus is ready.'),
      findsOneWidget,
    );
  });

  testWidgets('opens drive uri surfaced by a runtime cell result', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    final session = await client.createSession();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'start runtime');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'cell_result',
        id: 'cell-result-drive-uri',
        data: {
          'status': 'completed',
          'resultPreview':
              '{"filedUri":"drive://projects/tempestmiku/research/p5-drive-workspace.md"}',
        },
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _openActivitySheet(tester);
    final resultResource = find.byKey(
      const ValueKey(
        'activity-resource:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    expect(resultResource, findsOneWidget);
    await tester.tap(resultResource);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(find.textContaining('# Scripted drive note'), findsOneWidget);
  });

  testWidgets('renders structured runtime cell failures', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    final session = await client.createSession();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'start failed runtime');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'cell_start',
        id: 'cell-start-failed',
        data: {'cellId': 'cell-1', 'sourcePreview': '[redacted]'},
      ),
    );
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'cell_result',
        id: 'cell-result-failed',
        data: {
          'cellId': 'cell-1',
          'status': 'failed',
          'error': 'CapabilityDeniedError: [redacted]',
        },
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _openActivitySheet(tester);
    expect(find.text('執行程式'), findsOneWidget);
    expect(find.text('程式失敗'), findsWidgets);
    expect(find.text('[redacted]'), findsWidgets);
    expect(
      find.textContaining('CapabilityDeniedError: [redacted]'),
      findsWidgets,
    );
  });

  testWidgets('opens drive uri surfaced by a direct activity payload', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    final session = await client.createSession();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'file note');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'drive_put',
        id: 'drive-put-direct-uri',
        data: {
          'action': 'put',
          'uri': 'drive://projects/tempestmiku/research/p5-drive-workspace.md',
          'sourceUri': 'drop://browser/raw-research.md',
          'preview': {
            'title': 'Filed drive document',
            'subtitle': 'projects/tempestmiku/research/p5-drive-workspace.md',
            'snippet': 'Drive document content is ready.',
          },
        },
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _openActivitySheet(tester);
    final activityResource = find.byKey(
      const ValueKey(
        'activity-resource:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    expect(activityResource, findsOneWidget);
    expect(
      find.byKey(
        const ValueKey('activity-resource:drop://browser/raw-research.md'),
      ),
      findsNothing,
    );

    await tester.tap(activityResource);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(find.textContaining('# Scripted drive note'), findsOneWidget);
  });

  testWidgets('language switch toggles chrome without changing chat content', (
    WidgetTester tester,
  ) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('Miku is here'), findsOneWidget);
    expect(find.text('Miku 在這裡'), findsNothing);

    await _openSettings(tester);
    expect(find.text('Appearance and advanced actions'), findsOneWidget);
    await tester.tap(find.text('Language'));
    await tester.pump();
    await _popRoute(tester);

    expect(find.text('Miku 在這裡'), findsOneWidget);
    expect(find.text('Miku is here'), findsNothing);
    expect(find.bySemanticsLabel('送出訊息'), findsOneWidget);

    await tester.enterText(find.byType(EditableText), 'hello in any language');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('hello in any language'), findsOneWidget);
    expect(
      find.textContaining('Miku heard: hello in any language'),
      findsOneWidget,
    );
  });

  testWidgets(
    'shows remote control stream, final, hidden mode state, and project state',
    (WidgetTester tester) async {
      await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));

      expect(find.text('Tempest Miku'), findsOneWidget);
      expect(find.text('Personal'), findsNothing);
      expect(find.text('個人助理'), findsNothing);
      expect(find.text('燒烤'), findsNothing);
      expect(find.text('著陸'), findsNothing);
      expect(find.text('工程'), findsNothing);
      expect(find.text('交棒'), findsNothing);

      await tester.enterText(
        find.byType(EditableText),
        'please fix code artifact://0',
      );
      await tester.pump();
      await tester.tap(find.byIcon(Icons.send));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      expect(find.textContaining('認真工程師'), findsNothing);
      expect(find.text('Serious'), findsNothing);
      expect(find.text('燒烤'), findsNothing);
      expect(find.text('著陸'), findsNothing);
      expect(find.text('交棒'), findsNothing);
      expect(
        find.textContaining('Miku heard: please fix code artifact://0'),
        findsWidgets,
      );
      expect(find.text('artifact://0'), findsOneWidget);

      await tester.ensureVisible(find.text('artifact://0'));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('resource:artifact://0')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 350));

      expect(find.text('Scripted resource'), findsOneWidget);
      expect(find.text('Preview for artifact://0'), findsOneWidget);

      await tester.tap(find.byType(ModalBarrier).last);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 350));

      await _openSettings(tester);
      await tester.ensureVisible(find.text('Promote Session'));
      await tester.tap(find.text('Promote Session'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      await _openSettings(tester);
      await tester.ensureVisible(
        find.text('project://tempestmiku · 2 promoted'),
      );

      expect(find.text('project://tempestmiku · 2 promoted'), findsOneWidget);
      expect(find.text('Continue from latest session result'), findsOneWidget);
    },
  );

  testWidgets('records active conversation rounds in the thread', (
    WidgetTester tester,
  ) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'first status check');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await tester.enterText(find.byType(EditableText), 'second status check');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Round 1'), findsNothing);
    expect(find.text('Round 2'), findsNothing);
    expect(find.text('first status check'), findsOneWidget);
    expect(find.text('second status check'), findsOneWidget);
    expect(find.text('Miku heard: first status check'), findsOneWidget);
    expect(find.text('Miku heard: second status check'), findsOneWidget);
  });

  testWidgets(
    'opens session history, creates a new session, and restores one',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      await tester.pumpWidget(MikuApp(client: client));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));

      await tester.enterText(find.byType(EditableText), 'first history check');
      await tester.pump();
      await tester.tap(find.byIcon(Icons.send));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      await _selectDestination(tester, 'Sessions');
      expect(find.text('Sessions'), findsWidgets);
      expect(find.text('Miku heard: first history check'), findsWidgets);

      await tester.tap(find.bySemanticsLabel('Create new session'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(await client.listSessions(), hasLength(2));
      for (var i = 0; i < 20 && client.eventResumeIds.length < 2; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(client.eventResumeIds, hasLength(2));
      expect(find.text('Miku heard: first history check'), findsNothing);

      await tester.enterText(find.byType(EditableText), 'second history check');
      await tester.pump();
      await tester.tap(find.byIcon(Icons.send));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(find.text('second history check'), findsOneWidget);

      await _selectDestination(tester, 'Sessions');
      expect(find.text('Miku heard: first history check'), findsWidgets);
      expect(find.text('Miku heard: second history check'), findsWidgets);

      await tester.tap(find.text('Miku heard: first history check').last);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 350));
      expect(find.text('first history check'), findsOneWidget);
      expect(find.text('Miku heard: first history check'), findsOneWidget);
      expect(find.text('second history check'), findsNothing);
    },
  );

  testWidgets('shows selector from mode dropdown and exposes lock', (
    WidgetTester tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('個人助理'), findsNothing);
    expect(find.text('Personal locked'), findsNothing);
    expect(find.text('Personal'), findsNothing);

    await _openSettings(tester);
    await tester.ensureVisible(find.text('Mode settings'));
    await tester.tap(find.text('Mode settings'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Mode / Lock'), findsOneWidget);
    expect(find.text('Personal Assistant'), findsOneWidget);
    expect(find.text('Lock Personal'), findsOneWidget);

    await tester.tap(find.text('Serious Engineer'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.overriddenModes, contains('serious_engineer'));
    expect(find.text('Serious'), findsNothing);
    expect(find.text('認真工程師'), findsNothing);

    await _openSettings(tester);
    await tester.ensureVisible(find.text('Mode settings'));
    await tester.tap(find.text('Mode settings'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.pump(const Duration(milliseconds: 350));

    await tester.ensureVisible(find.text('Lock Serious'));
    await tester.pump();
    expect(find.text('Lock Serious'), findsOneWidget);
    await tester.tap(find.text('Lock Serious'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.lockedModes, contains('serious_engineer'));
    await _openSettings(tester);
    await tester.ensureVisible(find.text('Mode settings'));
    await tester.tap(find.text('Mode settings'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.pump(const Duration(milliseconds: 350));
    await tester.ensureVisible(find.text('Unlock Serious'));
    await tester.pump();
    expect(find.text('Unlock Serious'), findsOneWidget);
  });

  testWidgets('renders and resolves memory write proposals', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'remember this for me');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Memory proposal'), findsOneWidget);
    expect(
      find.text('Brian prefers approval-backed memory writes.'),
      findsOneWidget,
    );
    expect(find.text('scope global'), findsOneWidget);
    expect(find.text('provenance scripted chat turn'), findsOneWidget);
    expect(
      find.textContaining('Pending approval · memory.write'),
      findsNothing,
    );

    await tester.tap(find.text('Save memory'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(find.text('Memory proposal'), findsNothing);
  });

  testWidgets('phone view resolves a dream-origin memory proposal', (
    WidgetTester tester,
  ) async {
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'dream captured this');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Memory proposal'), findsOneWidget);
    expect(find.text('provenance post-session-dream'), findsOneWidget);
    await tester.tap(find.text('Save memory'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(find.text('Memory proposal'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('promotes actor completion resources from activity feed', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'handoff actor links');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _openActivitySheet(tester);

    final toolCall = find.text('呼叫工具 execute').last;
    final cellStart = find.text('執行程式').last;
    final actorCompleted = find.text('完成 Worker0').last;
    expect(toolCall, findsOneWidget);
    expect(cellStart, findsOneWidget);
    expect(actorCompleted, findsOneWidget);
    expect(find.textContaining('agents.spawn'), findsWidgets);
    expect(
      tester.getTopLeft(toolCall).dy,
      lessThan(tester.getTopLeft(cellStart).dy),
    );
    expect(
      tester.getTopLeft(cellStart).dy,
      lessThan(tester.getTopLeft(actorCompleted).dy),
    );

    final artifactLink = find.byKey(
      const ValueKey('activity-resource:artifact://0'),
    );
    final historyLink = find.byKey(
      const ValueKey('activity-resource:history://Worker0'),
    );
    expect(artifactLink, findsWidgets);
    expect(historyLink, findsWidgets);
    expect(find.text('artifact://0'), findsWidgets);
    expect(find.text('history://Worker0'), findsWidgets);

    await tester.ensureVisible(artifactLink.last);
    await tester.pump();
    await tester.tap(artifactLink.last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for artifact://0'), findsOneWidget);

    await _popRoute(tester);

    await tester.ensureVisible(historyLink.last);
    await tester.pump();
    await tester.tap(historyLink.last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for history://Worker0'), findsOneWidget);

    await _popRoute(tester);

    await _closeActivitySheet(tester);

    await _openSettings(tester);
    await tester.ensureVisible(find.text('Promote Session'));
    await tester.tap(find.text('Promote Session'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(
      client.promotedSummaries.single,
      'Actor Worker0 completed child resource artifact://0',
    );
    expect(client.promotedResources.single, [
      'artifact://0',
      'history://Worker0',
    ]);

    await _openSettings(tester);
    await tester.ensureVisible(find.text('project://tempestmiku · 3 promoted'));

    expect(find.text('project://tempestmiku · 3 promoted'), findsOneWidget);
  });

  testWidgets('keeps activity trace visible after final', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
      find.byType(EditableText),
      'handoff actor live trace',
    );
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    final activityCard = find.byKey(const ValueKey('agent-activity:1'));
    await _scrollChatUntilVisible(tester, activityCard);
    expect(activityCard, findsOneWidget);
    expect(find.text('Agents · 0 running / 1 stopped'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsNothing);

    client.completePausedTurn();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _scrollChatUntilVisible(tester, activityCard);
    expect(find.text('Agents · 0 running / 1 stopped'), findsOneWidget);
    expect(
      find.textContaining('Actor Worker0 completed', skipOffstage: false),
      findsOneWidget,
    );

    await _openActivitySheet(tester);

    expect(find.text('Prompt / Activity'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsWidgets);
    expect(find.text('執行程式'), findsWidgets);
    expect(find.text('完成 Worker0'), findsWidgets);
  });

  testWidgets('renders markdown and keeps reasoning visible after final', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
      find.byType(EditableText),
      'markdown with reasoning',
    );
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('P4 memo', findRichText: true), findsOneWidget);
    expect(find.text('•', findRichText: true), findsOneWidget);
    expect(find.text('☐', findRichText: true), findsOneWidget);
    expect(find.byType(RaTeXFormula), findsNWidgets(2));
    expect(find.text(r'\sin z = \frac{e^{iz}-e^{-iz}}{2i}'), findsOneWidget);
    expect(find.text(r'e^{i\pi}+1=0'), findsOneWidget);
    expect(find.textContaining(r'\\['), findsNothing);
    expect(find.text('Thinking'), findsOneWidget);
    final thinking = find.text('Thinking');
    await _scrollChatUntilVisible(tester, thinking);
    await tester.tap(
      find.ancestor(of: thinking, matching: find.byType(InkWell)).first,
    );
    await tester.pump();
    expect(
      find.textContaining(
        'Compare scheduler invariants',
        findRichText: true,
        skipOffstage: false,
      ),
      findsOneWidget,
    );
  });

  testWidgets('handles actor approval, child resource, and reconnect cursor', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'handoff actor approval');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Handoff'), findsNothing);
    final activityCard = find.byKey(const ValueKey('agent-activity:1'));
    await _scrollChatUntilVisible(tester, activityCard);
    expect(find.text('Agents · 0 running / 1 stopped'), findsOneWidget);
    expect(find.text('worker agent · Worker0'), findsOneWidget);
    expect(find.text('stopped'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsNothing);

    await _openActivitySheet(tester);

    expect(find.text('Agents · Round 1'), findsOneWidget);
    expect(find.text('Prompt / Activity'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsWidgets);
    expect(find.text('執行程式'), findsWidgets);
    expect(find.text('啟動 worker · Worker0'), findsWidgets);
    expect(find.text('完成 Worker0'), findsWidgets);
    expect(find.text('程式結果'), findsWidgets);

    await _closeActivitySheet(tester);

    final actorReply = find.textContaining(
      'Actor Worker0 completed',
      skipOffstage: false,
    );
    expect(actorReply, findsOneWidget);
    await tester.ensureVisible(actorReply);
    await tester.pump();
    expect(find.textContaining('Actor Worker0 completed'), findsOneWidget);
    expect(find.text('artifact://0'), findsWidgets);

    final answerArtifactLink = find.byKey(
      const ValueKey('resource:artifact://0'),
    );
    expect(answerArtifactLink, findsOneWidget);
    await tester.ensureVisible(answerArtifactLink);
    await tester.pump();
    await tester.tap(answerArtifactLink);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for artifact://0'), findsOneWidget);

    await tester.tap(find.byType(ModalBarrier).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    final approvalCard = find.byKey(
      const ValueKey('approval:proc.run cargo clean'),
    );
    await _scrollChatUntilVisible(tester, approvalCard);
    expect(approvalCard, findsOneWidget);
    await tester.ensureVisible(approvalCard);
    await tester.pump();
    await tester.tap(approvalCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('actorId: Worker0'), findsOneWidget);
    await tester.tap(find.text('Approve once'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(approvalCard, findsNothing);

    final remembered = client.rememberedLastEventIds.values.single;
    await tester.pumpWidget(MikuApp(key: UniqueKey(), client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(client.eventResumeIds.last, remembered);
  });
}

class _FailOnceMikuClient extends ScriptedMikuClient {
  final List<String> attemptedClientMessageIds = [];
  var _failed = false;

  @override
  Future<void> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) async {
    attemptedClientMessageIds.add(clientMessageId);
    if (!_failed) {
      _failed = true;
      throw StateError('simulated send failure');
    }
    await super.sendMessage(
      sessionId,
      content,
      clientMessageId: clientMessageId,
    );
  }
}

class _ControlledSendMikuClient extends ScriptedMikuClient {
  final Completer<void> _slowSendGate = Completer<void>();
  var slowSendStarted = false;
  var slowSendCompleted = false;

  void completeSlowSend() {
    if (!_slowSendGate.isCompleted) _slowSendGate.complete();
  }

  @override
  Future<void> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) async {
    if (content == 'slow message from A') {
      slowSendStarted = true;
      await _slowSendGate.future;
    }
    await super.sendMessage(
      sessionId,
      content,
      clientMessageId: clientMessageId,
    );
    if (content == 'slow message from A') slowSendCompleted = true;
  }
}

class _SlowInitialConnectMikuClient extends ScriptedMikuClient {
  final Completer<MikuSession> _initialSession = Completer<MikuSession>();
  var initialConnectRequested = false;
  var explicitCreateRequests = 0;

  @override
  Future<MikuSession> createOrReuseSession() {
    initialConnectRequested = true;
    return _initialSession.future;
  }

  @override
  Future<MikuSession> createSession() {
    explicitCreateRequests += 1;
    return super.createSession();
  }

  Future<void> completeInitialConnect() async {
    if (_initialSession.isCompleted) return;
    _initialSession.complete(await super.createSession());
  }
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
