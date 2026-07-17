part of 'widget_test.dart';

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
  await tester.tap(find.byTooltip('Open menu').first);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
  await tester.tap(find.text('Settings').first);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 100));
}

Future<void> _openContext(WidgetTester tester) async {
  await tester.tap(find.byTooltip('Open menu').first);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
  await tester.tap(find.text('Context').first);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 100));
}

Future<void> _scrollDrawerUntilVisible(
  WidgetTester tester,
  Finder target,
) async {
  final scrollables = find.descendant(
    of: find.byType(Drawer),
    matching: find.byType(Scrollable),
  );
  await tester.scrollUntilVisible(
    target,
    220,
    scrollable: scrollables.last,
    maxScrolls: 12,
  );
  await tester.pump();
}

Future<void> _tapDrawerAction(WidgetTester tester, String label) async {
  await _openSettings(tester);
  final action = find.text(label);
  await _scrollDrawerUntilVisible(tester, action);
  await tester.tap(action);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
}

Future<void> _startNewSessionFromDrawer(WidgetTester tester) async {
  await tester.tap(find.text('New session').first);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
}

Future<void> _selectDestination(WidgetTester tester, String label) async {
  if (label == 'Chat') {
    await _popRoute(tester);
    return;
  }
  await tester.tap(find.byTooltip('Open menu').first);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
  if (label == 'Sessions') return;
  await tester.tap(find.text('Settings').first);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 100));
  final destination = find.text(label);
  await _scrollDrawerUntilVisible(tester, destination);
  await tester.tap(destination);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 700));
  await tester.pump(const Duration(milliseconds: 350));
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
