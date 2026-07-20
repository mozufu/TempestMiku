import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/pairing_scanner.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';
import 'package:miku_flutter/share_import_service.dart';

void main() {
  Widget appFor(
    ScriptedMikuClient client, {
    MikuShareImportService? shareImports,
  }) => TempestMikuApp(
    client: client,
    themeMode: ThemeMode.light,
    now: () => DateTime(2026, 7, 19, 20),
    shareImports: shareImports,
  );

  Future<void> loadApp(
    WidgetTester tester,
    ScriptedMikuClient client, {
    MikuShareImportService? shareImports,
  }) async {
    await tester.pumpWidget(appFor(client, shareImports: shareImports));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
  }

  testWidgets('starts as a quiet, present conversation canvas', (tester) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);

    expect(find.text('Miku'), findsOneWidget);
    expect(find.text('伺服器已連線'), findsOneWidget);
    expect(find.text('晚上好。我在這裡。'), findsOneWidget);
    expect(find.byKey(const Key('conversation-composer')), findsOneWidget);
    expect(find.byTooltip('送出'), findsOneWidget);
    expect(find.byTooltip('開啟對話選單'), findsOneWidget);
    expect(find.byIcon(Icons.attach_file), findsNothing);
  });

  testWidgets('opens the left conversation drawer by swiping right', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 667);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    expect(await client.listSessions(), hasLength(1));
    final scaffold = tester.state<ScaffoldState>(find.byType(Scaffold));
    expect(scaffold.isDrawerOpen, isFalse);

    await tester.dragFrom(const Offset(1, 280), const Offset(300, 0));
    await tester.pumpAndSettle();

    expect(scaffold.isDrawerOpen, isTrue);
    expect(find.byKey(const Key('left-conversation-drawer')), findsOneWidget);
    expect(find.byKey(const Key('left-drawer-title')), findsOneWidget);
    expect(find.text('Miku'), findsWidgets);
    expect(find.byKey(const Key('drawer-drive')), findsOneWidget);
    expect(find.byKey(const Key('drawer-project')), findsOneWidget);
    expect(find.byKey(const Key('drawer-history')), findsOneWidget);
    expect(find.byKey(const Key('drawer-settings')), findsOneWidget);
    expect(find.byKey(const Key('drawer-new-conversation')), findsOneWidget);

    await tester.tap(find.byKey(const Key('close-left-drawer')));
    await tester.pumpAndSettle();
    expect(scaffold.isDrawerOpen, isFalse);

    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    expect(scaffold.isDrawerOpen, isTrue);

    await tester.tap(find.byKey(const Key('drawer-new-conversation')));
    await tester.pumpAndSettle();
    expect(scaffold.isDrawerOpen, isFalse);
    expect(await client.listSessions(), hasLength(2));
  });

  testWidgets('opens session context and changes then locks Mode', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    await loadApp(tester, client);

    await tester.tap(find.byKey(const Key('open-session-context')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('session-context-drawer')), findsOneWidget);
    expect(find.byKey(const Key('mode-personal_assistant')), findsOneWidget);
    expect(find.byKey(const Key('mode-serious_engineer')), findsOneWidget);

    await tester.tap(find.byKey(const Key('mode-serious_engineer')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    expect(client.overriddenModes, ['serious_engineer']);
    final serious = tester.widget<ListTile>(
      find.byKey(const Key('mode-serious_engineer')),
    );
    expect(serious.selected, isTrue);

    await tester.scrollUntilVisible(
      find.byKey(const Key('mode-lock-toggle')),
      180,
      scrollable:
          find
              .descendant(
                of: find.byKey(const Key('session-context-drawer')),
                matching: find.byType(Scrollable),
              )
              .first,
    );
    await tester.tap(find.byKey(const Key('mode-lock-toggle')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    expect(client.lockedModes, ['serious_engineer']);
    final lock = tester.widget<SwitchListTile>(
      find.byKey(const Key('mode-lock-toggle')),
    );
    expect(lock.value, isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets('shows typed memory and evolution review details in approvals', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await client.createSession();
    client.seedPendingApproval(
      session.id,
      approvalId: 'memory-review',
      action: 'memory.write profile_fact',
      backend: 'memory',
      scope: const {
        'proposal': {
          'kind': 'memory',
          'proposalId': 'memory-proposal',
          'memoryKind': 'profile_fact',
          'preview': '偏好繁體中文介面',
          'uri': 'memory://evolution-proposals/memory-proposal',
          'contentDigest': 'sha256:scripted',
          'recordId': 'record-scripted',
        },
      },
    );
    client.seedPendingApproval(
      session.id,
      approvalId: 'persona-review',
      action: 'review persona addendum miku',
      backend: 'evolution-review',
      scope: const {
        'kind': 'evolution_review',
        'proposalId': 'persona-proposal',
        'target': {'kind': 'persona', 'personaId': 'miku'},
        'preview': 'tone: 更精簡但保留角色感',
        'uri': 'memory://review-proposals/persona-proposal',
        'applyEnabled': true,
      },
    );

    await loadApp(tester, client);
    expect(find.byKey(const Key('memory-proposal-details')), findsOneWidget);
    expect(find.text('偏好繁體中文介面'), findsOneWidget);
    expect(find.text('來源：full proposal resource'), findsOneWidget);
    expect(find.byKey(const Key('evolution-proposal-details')), findsOneWidget);
    expect(find.text('Persona · miku'), findsOneWidget);
    expect(find.text('核准後會建立不可變版本並啟用。'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('settings shows diagnostics, revokes a device, and logs out', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-settings')));
    await tester.pumpAndSettle();

    expect(find.byKey(const Key('settings-title')), findsOneWidget);
    await tester.scrollUntilVisible(
      find.byKey(const Key('server-diagnostics')),
      180,
      scrollable:
          find
              .descendant(
                of: find.byKey(const Key('settings-sheet')),
                matching: find.byType(Scrollable),
              )
              .first,
    );
    expect(find.byKey(const Key('server-diagnostics')), findsOneWidget);
    expect(find.text('伺服器已就緒'), findsOneWidget);
    expect(find.text('https://miku.example'), findsOneWidget);
    await tester.scrollUntilVisible(
      find.byKey(const Key('auth-device-device-browser')),
      180,
      scrollable:
          find
              .descendant(
                of: find.byKey(const Key('settings-sheet')),
                matching: find.byType(Scrollable),
              )
              .first,
    );
    expect(find.byKey(const Key('auth-device-device-browser')), findsOneWidget);
    expect(find.byKey(const Key('current-auth-device')), findsOneWidget);
    expect(find.byTooltip('撤銷 TempestMiku scripted'), findsNothing);

    await tester.tap(find.byKey(const Key('create-pairing-code')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('配對新裝置'), findsOneWidget);
    expect(find.byKey(const Key('pairing-link')), findsOneWidget);
    await tester.tap(find.byKey(const Key('copy-pairing-link')));
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, '完成'));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('撤銷 Laptop browser'));
    await tester.pumpAndSettle();
    expect(find.text('撤銷裝置？'), findsOneWidget);
    await tester.tap(find.byKey(const Key('confirm-device-revoke')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    expect(client.revokedDeviceIds, ['device-browser']);
    expect(find.byKey(const Key('auth-device-device-browser')), findsNothing);
    await tester.tap(find.byTooltip('重新整理裝置'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('auth-device-device-browser')), findsOneWidget);
    expect(find.text('已撤銷'), findsOneWidget);

    await tester.scrollUntilVisible(
      find.byKey(const Key('logout-device')),
      160,
      scrollable:
          find
              .descendant(
                of: find.byKey(const Key('settings-sheet')),
                matching: find.byType(Scrollable),
              )
              .first,
    );
    await tester.drag(
      find.byKey(const Key('settings-list')),
      const Offset(0, -120),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('logout-device')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('confirm-logout')));
    await tester.pumpAndSettle();
    expect(client.logoutCount, 1);
    expect(find.textContaining('已登出這台裝置'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('pairs this device only after reviewing the server target', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-settings')));
    await tester.pumpAndSettle();

    const pairingLink =
        'tempestmiku://pair?v=1&server=https%3A%2F%2Fnew-miku.example%3A8443&code=aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899';
    await tester.enterText(
      find.byKey(const Key('pairing-link-input')),
      pairingLink,
    );
    await tester.tap(find.byKey(const Key('pair-this-device')));
    await tester.pumpAndSettle();

    expect(find.text('確認配對目標'), findsOneWidget);
    expect(find.text('HTTPS'), findsOneWidget);
    expect(find.text('new-miku.example'), findsOneWidget);
    expect(find.text('8443'), findsOneWidget);
    expect(find.text('TempestMiku scripted'), findsOneWidget);
    expect(find.byKey(const Key('pairing-target-origin')), findsOneWidget);

    await tester.tap(find.byKey(const Key('confirm-pair-device')));
    await tester.pumpAndSettle();

    expect(client.pairedTargets, hasLength(1));
    expect(
      client.pairedTargets.single.serverBaseUrl,
      'https://new-miku.example:8443',
    );
    expect(find.byKey(const Key('settings-sheet')), findsNothing);
    expect(find.byKey(const Key('conversation-composer')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('camera QR only fills the reviewed pairing flow', (tester) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-settings')));
    await tester.pumpAndSettle();

    const pairingLink =
        'tempestmiku://pair?v=1&server=https%3A%2F%2Fnew-miku.example%3A8443&code=aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899';
    final scanButton = find.byKey(const Key('scan-pairing-qr'));
    expect(scanButton, findsOneWidget);
    expect(tester.getSize(scanButton).height, greaterThanOrEqualTo(48));

    await tester.tap(scanButton);
    await tester.pump();
    expect(find.byType(PairingScannerPage), findsOneWidget);
    Navigator.of(
      tester.element(find.byType(PairingScannerPage)),
    ).pop(pairingLink);
    await tester.pumpAndSettle();

    expect(
      tester
          .widget<TextField>(find.byKey(const Key('pairing-link-input')))
          .controller
          ?.text,
      pairingLink,
    );
    expect(find.text('已讀取 QR；尚未配對。請檢查目標後再確認。'), findsOneWidget);
    expect(client.pairedTargets, isEmpty);

    await tester.tap(find.byKey(const Key('pair-this-device')));
    await tester.pumpAndSettle();
    expect(find.text('確認配對目標'), findsOneWidget);
    expect(client.pairedTargets, isEmpty);
    await tester.tap(find.widgetWithText(TextButton, '取消'));
    await tester.pumpAndSettle();
    expect(client.pairedTargets, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'resource inspector lists registered schemes and bounded previews',
    (tester) async {
      tester.view.physicalSize = const Size(375, 812);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      final client = ScriptedMikuClient();
      await loadApp(tester, client);
      await tester.tap(find.byKey(const Key('open-left-drawer')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('drawer-resources')));
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('resource-inspector')), findsOneWidget);
      expect(
        find.byKey(const Key('resource-entry-artifact://')),
        findsOneWidget,
      );
      expect(find.byKey(const Key('resource-entry-memory://')), findsOneWidget);
      expect(find.byKey(const Key('resource-entry-skill://')), findsOneWidget);

      await tester.tap(find.byKey(const Key('open-exact-resource-uri')));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('exact-resource-uri-input')),
        'history://scripted-actor',
      );
      await tester.tap(find.byKey(const Key('confirm-exact-resource-uri')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('resource-preview-content')), findsOneWidget);
      expect(
        find.textContaining('Preview for history://scripted-actor'),
        findsOneWidget,
      );
      await tester.tap(find.byKey(const Key('resource-back')));
      await tester.pumpAndSettle();

      await tester.tap(find.byKey(const Key('resource-entry-artifact://')));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const Key('resource-entry-artifact://scripted-report')),
        findsOneWidget,
      );

      await tester.tap(
        find.byKey(const Key('resource-entry-artifact://scripted-report')),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('resource-preview-content')), findsOneWidget);
      expect(
        find.textContaining('Preview for artifact://scripted-report'),
        findsOneWidget,
      );
      await tester.tap(find.byKey(const Key('resource-back')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('resource-back')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('resource-entry-skill://')));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const Key('resource-entry-skill://scripted-skill')),
      );
      await tester.pumpAndSettle();
      expect(
        find.byKey(
          const Key(
            'resource-entry-skill://scripted-skill/versions/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
          ),
        ),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('reviews shared text before an explicit current-session send', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final imports = _FakeShareImportService();
    addTearDown(imports.close);
    await loadApp(tester, client, shareImports: imports);
    await tester.enterText(
      find.byKey(const Key('conversation-composer')),
      '原本還在整理的草稿',
    );

    imports.add(
      const SharedContent(
        text: 'Original shared text',
        source: SharedContentSource.selection,
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byKey(const Key('import-review-sheet')), findsOneWidget);
    expect(find.textContaining('不會自動送出'), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(find.byKey(const Key('send-import')))
          .onPressed,
      isNull,
    );
    await tester.enterText(
      find.byKey(const Key('import-review-editor')),
      'Edited shared text',
    );
    await tester.tap(find.text('目前對話'));
    await tester.pump();
    await tester.tap(find.byKey(const Key('send-import')));
    await tester.pumpAndSettle();

    final session = await client.createOrReuseSession();
    final loaded = await client.loadSession(session.id);
    expect(
      loaded.messages.where((message) => message.role == 'user').last.content,
      'Edited shared text',
    );
    expect(client.sentClientMessageIds, hasLength(1));
    expect(find.byKey(const Key('import-review-sheet')), findsNothing);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('conversation-composer')))
          .controller!
          .text,
      '原本還在整理的草稿',
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('replaces a warm quick-capture draft and requires a new choice', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final imports = _FakeShareImportService();
    addTearDown(imports.close);
    await loadApp(tester, client, shareImports: imports);

    imports.add(
      const SharedContent(
        text: 'first',
        source: SharedContentSource.quickCapture,
        eventId: '11111111-1111-4111-8111-111111111111',
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('目前對話'));
    await tester.pump();
    expect(
      tester
          .widget<FilledButton>(find.byKey(const Key('send-import')))
          .onPressed,
      isNotNull,
    );

    imports.add(
      const SharedContent(
        text: 'newest',
        source: SharedContentSource.quickCapture,
        eventId: '22222222-2222-4222-8222-222222222222',
      ),
    );
    await tester.pump();

    final editor = tester.widget<TextField>(
      find.byKey(const Key('import-review-editor')),
    );
    expect(editor.controller!.text, 'newest');
    expect(
      tester
          .widget<FilledButton>(find.byKey(const Key('send-import')))
          .onPressed,
      isNull,
    );
    await tester.tap(find.byTooltip('取消匯入'));
    await tester.pumpAndSettle();
    expect(client.sentClientMessageIds, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('ends the current session through an explicit confirmation', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    final session = await client.createOrReuseSession();
    await tester.tap(find.byKey(const Key('open-session-context')));
    await tester.pumpAndSettle();
    await tester.scrollUntilVisible(
      find.byKey(const Key('end-session')),
      220,
      scrollable:
          find
              .descendant(
                of: find.byKey(const Key('session-context-drawer')),
                matching: find.byType(Scrollable),
              )
              .first,
    );
    await tester.tap(find.byKey(const Key('end-session')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('confirm-end-session')));
    await tester.pumpAndSettle();

    expect((await client.loadSession(session.id)).session.status, 'ended');
    final composer = tester.widget<TextField>(
      find.byKey(const Key('conversation-composer')),
    );
    expect(composer.enabled, isFalse);
    expect(find.text('對話已結束'), findsWidgets);
    expect(tester.takeException(), isNull);
  });

  testWidgets('opens scoped Drive playground and previews a bounded document', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    final session = await client.createSession();
    await client.setSessionScope(session.id, 'project:tempestmiku');
    await client.sendMessage(
      session.id,
      '整理 Drive research',
      clientMessageId: 'seed-drive-workspace',
    );
    await loadApp(tester, client);

    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-drive')));
    await tester.pumpAndSettle();

    const documentUri =
        'drive://projects/tempestmiku/research/p5-drive-workspace.md';
    expect(find.byKey(const Key('drive-page-content')), findsOneWidget);
    expect(
      find.byKey(const Key('drive-document-$documentUri')),
      findsOneWidget,
    );
    expect(
      find.byKey(const Key('drive-proposal-drive-proposal-scripted')),
      findsOneWidget,
    );
    expect(client.driveFeedRequests, 1);

    await tester.ensureVisible(
      find.byKey(const Key('drive-document-$documentUri')),
    );
    await tester.tap(find.byKey(const Key('drive-document-$documentUri')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('drive-preview-title')), findsOneWidget);
    expect(find.byKey(const Key('drive-preview-content')), findsOneWidget);
    expect(find.textContaining('Local citation corpus'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Drive is a global playground and exposes a retryable error', (
    tester,
  ) async {
    // §30: drive is Miku's playground, reachable without a project. A global session opens the
    // page and loads the unprojected feed instead of demanding a project scope first.
    final globalClient = ScriptedMikuClient();
    await loadApp(tester, globalClient);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-drive')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('drive-page-content')), findsOneWidget);
    expect(find.text('Miku 的空間'), findsOneWidget);
    expect(globalClient.driveFeedRequests, 1);

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();
    final failingClient = ScriptedMikuClient(failDriveFeed: true);
    final session = await failingClient.createSession();
    await failingClient.setSessionScope(session.id, 'project:tempestmiku');
    await loadApp(tester, failingClient);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-drive')));
    await tester.pumpAndSettle();
    expect(find.text('Drive 暫時讀不到，請再試一次。'), findsOneWidget);

    failingClient.failDriveFeed = false;
    await tester.tap(find.byKey(const Key('drive-refresh')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('drive-page-content')), findsOneWidget);
  });

  testWidgets('opens project page then history page and switches sessions', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    final first = await client.createSession();
    await client.sendMessage(
      first.id,
      '第一段對話',
      clientMessageId: 'history-first-message',
    );
    final second = await client.createSession();
    await loadApp(tester, client);

    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('project-tempestmiku')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('project-page-content')), findsOneWidget);
    expect(find.text('Scripted project status'), findsOneWidget);
    expect(
      find.textContaining('Continue from latest session result'),
      findsOneWidget,
    );

    // Back out of the project page, then open History from the drawer.
    await tester.pageBack();
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-history')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('history-page-content')), findsOneWidget);
    expect(find.byKey(Key('history-session-${first.id}')), findsOneWidget);
    expect(find.byKey(Key('history-session-${second.id}')), findsOneWidget);

    await tester.ensureVisible(find.byKey(Key('history-session-${first.id}')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(Key('history-session-${first.id}')));
    await tester.pumpAndSettle();

    final scaffold = tester.state<ScaffoldState>(find.byType(Scaffold));
    expect(scaffold.isDrawerOpen, isFalse);
    expect(find.text('第一段對話'), findsOneWidget);
    expect((await client.createOrReuseSession()).id, first.id);
  });

  testWidgets('assigns only a closed history session to a project', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final closed = await client.createSession();
    await client.sendMessage(
      closed.id,
      '封存後指派',
      clientMessageId: 'closed-history-assignment',
    );
    client.endSessionForTesting(closed.id);
    final active = await client.createSession();
    await loadApp(tester, client);

    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-history')));
    await tester.pumpAndSettle();

    expect(find.byKey(Key('history-assign-${active.id}')), findsNothing);
    await tester.tap(find.byKey(Key('history-assign-${closed.id}')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('assign-project-tempestmiku')));
    await tester.pumpAndSettle();

    expect(client.assignedSessionIds, [closed.id]);
    expect(client.assignedProjectIds, ['tempestmiku']);
    expect(find.textContaining('成長了 3 個 Project 項目'), findsOneWidget);
  });

  testWidgets(
    'returns a project-scoped conversation to Global without losing content',
    (tester) async {
      tester.view.physicalSize = const Size(375, 812);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      final client = ScriptedMikuClient();
      final session = await client.createSession();
      await client.setSessionScope(session.id, 'project:tempestmiku');
      await loadApp(tester, client);

      await tester.enterText(
        find.byKey(const Key('conversation-composer')),
        '保留這段訊息',
      );
      await tester.pump();
      await tester.tap(find.byKey(const Key('send-message')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 250));
      await tester.enterText(
        find.byKey(const Key('conversation-composer')),
        '尚未送出的草稿',
      );

      await tester.tap(find.byKey(const Key('open-left-drawer')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('drawer-project')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('project-browser-up')));
      await tester.pumpAndSettle();

      final globalScope = find.byKey(const Key('project-global-scope'));
      expect(globalScope, findsOneWidget);
      expect(tester.getSize(globalScope).height, greaterThanOrEqualTo(44));
      expect(tester.widget<ListTile>(globalScope).selected, isFalse);

      await tester.tap(globalScope);
      await tester.pumpAndSettle();

      expect(
        (await client.loadSession(session.id)).session.defaultScope,
        'global',
      );
      expect(tester.widget<ListTile>(globalScope).selected, isTrue);
      expect(
        tester
            .widget<ListTile>(find.byKey(const Key('project-tempestmiku')))
            .selected,
        isFalse,
      );

      await tester.pageBack();
      await tester.pumpAndSettle();
      expect(find.text('保留這段訊息'), findsOneWidget);
      expect(find.text('Miku heard: 保留這段訊息'), findsOneWidget);
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('conversation-composer')))
            .controller!
            .text,
        '尚未送出的草稿',
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('opens a flat project root and previews bounded file content', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('project-tempestmiku')));
    await tester.pumpAndSettle();

    const linkedRoot = 'project://tempestmiku/linked-folders/tempestmiku/';
    const docs = 'project://tempestmiku/linked-folders/tempestmiku/docs/';
    const readme = 'project://tempestmiku/linked-folders/tempestmiku/README.md';
    const symlink = 'project://tempestmiku/linked-folders/tempestmiku/latest';
    expect(
      find.byKey(const Key('project-resource-$linkedRoot')),
      findsNothing,
      reason: 'The linked-folder collection is not a visible project level.',
    );
    expect(find.byKey(const Key('project-resource-$docs')), findsOneWidget);
    expect(find.byKey(const Key('project-resource-$readme')), findsOneWidget);
    final unsupported = tester.widget<ListTile>(
      find.byKey(const Key('project-resource-$symlink')),
    );
    expect(unsupported.enabled, isFalse);

    await tester.tap(find.byKey(const Key('project-resource-$docs')));
    await tester.pumpAndSettle();
    expect(find.text('guide.md'), findsOneWidget);
    await tester.tap(find.byKey(const Key('project-browser-up')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('project-resource-$readme')), findsOneWidget);

    await tester.tap(find.byKey(const Key('project-resource-$readme')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('project-file-title')), findsOneWidget);
    expect(find.byKey(const Key('project-file-content')), findsOneWidget);
    expect(find.byKey(const Key('project-file-truncated')), findsOneWidget);
    expect(find.textContaining('Scripted linked resource'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('project page shows auto-grown items without a promote action', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    final session = await client.createSession();
    await client.setSessionScope(session.id, 'project:tempestmiku');
    await client.sendMessage(
      session.id,
      'Summarize the project update',
      clientMessageId: 'project-items-message',
    );
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();

    // §30: items grow from the session's turns; there is no promote affordance.
    expect(find.text('Open loops'), findsOneWidget);
    expect(find.text('Decisions'), findsOneWidget);
    expect(find.byKey(const Key('promote-session')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('creates a folderless project and assigns the active session', (
    tester,
  ) async {
    // §30.2/§30.6: a project is a first-class entity that can exist without a folder; an active
    // session declares its project through the scope endpoint, not the closed-session assignment API.
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    final session = await client.createSession();
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const Key('project-create')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('create-project-title')),
      '旅遊規劃',
    );
    await tester.tap(find.byKey(const Key('create-project-submit')));
    await tester.pumpAndSettle();

    final created = client.createdProjects.single;
    expect(created.title, '旅遊規劃');
    expect(created.hasLinkedFolder, isFalse);
    expect(created.id, startsWith('project-'));
    expect(
      (await client.loadSession(session.id)).session.defaultScope,
      created.memoryScope,
    );
  });

  testWidgets('archives a project entity and removes it from the picker', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const Key('project-archive-tempestmiku')));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, '封存'));
    await tester.pumpAndSettle();

    expect(client.archivedProjectIds, ['tempestmiku']);
    expect(find.byKey(const Key('project-tempestmiku')), findsNothing);
  });

  testWidgets('shows project catalog empty and retryable error states', (
    tester,
  ) async {
    final emptyClient = ScriptedMikuClient(projectCatalogEmpty: true);
    await loadApp(tester, emptyClient);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    expect(find.text('尚未建立任何 Project。用右上角的「＋」新增一個。'), findsOneWidget);

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();
    final failingClient = ScriptedMikuClient(failProjectCatalog: true);
    await loadApp(tester, failingClient);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    expect(find.text('Project 暫時讀不到，請再試一次。'), findsOneWidget);

    failingClient.failProjectCatalog = false;
    await tester.tap(find.byTooltip('重試'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('project-tempestmiku')), findsOneWidget);
  });

  testWidgets('does not change active project when scope switch fails', (
    tester,
  ) async {
    final client = ScriptedMikuClient(failProjectScope: true);
    await loadApp(tester, client);
    final session = await client.createOrReuseSession();
    expect(session.defaultScope, 'global');

    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('project-tempestmiku')));
    await tester.pumpAndSettle();

    expect(find.text('Project 暫時讀不到，請再試一次。'), findsOneWidget);
    expect((await client.createOrReuseSession()).defaultScope, 'global');
  });

  testWidgets(
    'ended session keeps its project browsable but blocks switching',
    (tester) async {
      final client = ScriptedMikuClient(includeArchiveProject: true);
      final session = await client.createSession();
      await client.setSessionScope(session.id, 'project:tempestmiku');
      client.endSessionForTesting(session.id);
      await loadApp(tester, client);
      await tester.tap(find.byKey(const Key('open-left-drawer')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('drawer-project')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('project-browser-up')));
      await tester.pumpAndSettle();

      final active = tester.widget<ListTile>(
        find.byKey(const Key('project-tempestmiku')),
      );
      final archive = tester.widget<ListTile>(
        find.byKey(const Key('project-archive')),
      );
      expect(active.enabled, isTrue);
      expect(archive.enabled, isFalse);
      expect(find.text('請先開新對話'), findsOneWidget);
    },
  );

  testWidgets('retries folder listing and reports file resolve errors', (
    tester,
  ) async {
    final client = ScriptedMikuClient(failProjectResources: true);
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('project-tempestmiku')));
    await tester.pumpAndSettle();
    expect(find.text('Project 暫時讀不到，請再試一次。'), findsOneWidget);

    client.failProjectResources = false;
    await tester.tap(find.byTooltip('重試'));
    await tester.pumpAndSettle();

    client.failProjectResolve = true;
    const readme = 'project://tempestmiku/linked-folders/tempestmiku/README.md';
    await tester.drag(
      find.byKey(const Key('project-page-content')),
      const Offset(0, -220),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('project-resource-$readme')));
    await tester.pumpAndSettle();
    expect(find.text('Project 暫時讀不到，請再試一次。'), findsOneWidget);
    expect(find.byKey(const Key('project-file-content')), findsNothing);
  });

  testWidgets('keeps the enabled send arrow high contrast in dark mode', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(
      TempestMikuApp(
        client: client,
        themeMode: ThemeMode.dark,
        now: () => DateTime(2026, 7, 19, 20),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
      find.byKey(const Key('conversation-composer')),
      '有足夠對比的送出按鈕',
    );
    await tester.pump();

    final sendButton = tester.widget<IconButton>(
      find.byKey(const Key('send-message')),
    );
    final colors =
        Theme.of(
          tester.element(find.byKey(const Key('send-message'))),
        ).colorScheme;
    final background =
        sendButton.style!.backgroundColor!.resolve(const <WidgetState>{})!;
    final foreground =
        sendButton.style!.foregroundColor!.resolve(const <WidgetState>{})!;

    expect(background, colors.primary);
    expect(foreground, colors.onPrimary);
    expect(_contrastRatio(background, foreground), greaterThanOrEqualTo(4.5));
  });

  testWidgets('sends a message and renders Miku directly on the canvas', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);

    await tester.enterText(
      find.byKey(const Key('conversation-composer')),
      '今天陪我整理一下',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('send-message')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));

    expect(find.text('今天陪我整理一下'), findsOneWidget);
    expect(find.text('Miku heard: 今天陪我整理一下'), findsOneWidget);
    expect(client.sentClientMessageIds, hasLength(1));

    final assistantText = tester.widget<SelectableText>(
      find.widgetWithText(SelectableText, 'Miku heard: 今天陪我整理一下'),
    );
    final ancestor = find.ancestor(
      of: find.byWidget(assistantText),
      matching: find.byType(DecoratedBox),
    );
    expect(ancestor, findsNothing);
  });

  testWidgets('keeps a streamed response visibly active until final', (
    tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    await loadApp(tester, client);

    await tester.enterText(
      find.byKey(const Key('conversation-composer')),
      '慢慢回答',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('send-message')));
    await tester.pump();

    expect(find.text('Miku heard: 慢慢回答'), findsOneWidget);
    expect(find.text('伺服器已連線'), findsOneWidget);
    expect(find.text('Miku 正在處理'), findsOneWidget);

    client.completePausedTurn();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));

    expect(find.text('伺服器已連線'), findsOneWidget);
    expect(find.text('Miku heard: 慢慢回答'), findsOneWidget);
    expect(find.text('已完成並保存'), findsOneWidget);
  });

  testWidgets('shows approval in the conversation and resolves it inline', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);

    await tester.enterText(
      find.byKey(const Key('conversation-composer')),
      '請 actor 幫忙',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('send-message')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));

    expect(find.text('需要你的確認'), findsOneWidget);
    expect(find.text('proc.run cargo clean'), findsOneWidget);
    expect(find.byKey(const Key('approval-option-allow')), findsOneWidget);

    await tester.ensureVisible(find.byKey(const Key('approval-option-allow')));
    await tester.pump();
    await tester.tap(find.byKey(const Key('approval-option-allow')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, contains(':approve'));
    expect(find.text('已允許'), findsOneWidget);
  });

  testWidgets('fits a compact phone viewport without overflow', (tester) async {
    tester.view.physicalSize = const Size(375, 667);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = ScriptedMikuClient();
    await loadApp(tester, client);

    expect(tester.takeException(), isNull);
    expect(find.byKey(const Key('empty-presence-copy')), findsOneWidget);
    expect(find.byKey(const Key('conversation-composer')), findsOneWidget);
  });

  testWidgets('makes a terminal conversation clearly read-only', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    final session = await client.createOrReuseSession();

    client.emitEvent(
      session.id,
      const MikuEvent(type: 'session_end', id: 'event-terminal', data: {}),
    );
    await tester.pump();

    expect(find.text('這段對話已結束'), findsWidgets);
    final composer = tester.widget<TextField>(
      find.byKey(const Key('conversation-composer')),
    );
    expect(composer.enabled, isFalse);
    await tester.tap(find.byKey(const Key('open-session-context')));
    await tester.pumpAndSettle();
    expect(find.text('已結束的對話'), findsOneWidget);
    await tester.scrollUntilVisible(
      find.byKey(const Key('end-session')),
      220,
      scrollable:
          find
              .descendant(
                of: find.byKey(const Key('session-context-drawer')),
                matching: find.byType(Scrollable),
              )
              .first,
    );
    expect(
      tester
          .widget<OutlinedButton>(find.byKey(const Key('end-session')))
          .onPressed,
      isNull,
    );
  });

  testWidgets('keeps reconnecting visible and restores the composer', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    final session = await client.createOrReuseSession();

    client.emitEvent(
      session.id,
      const MikuEvent(type: 'connection', data: {'status': 'reconnecting'}),
    );
    await tester.pump();

    expect(find.text('正在重新連線'), findsOneWidget);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('conversation-composer')))
          .enabled,
      isFalse,
    );

    client.emitEvent(
      session.id,
      const MikuEvent(type: 'connection', data: {'status': 'connected'}),
    );
    await tester.pump();

    expect(find.text('伺服器已連線'), findsOneWidget);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('conversation-composer')))
          .enabled,
      isTrue,
    );

    client.emitEvent(
      session.id,
      const MikuEvent(type: 'connection', data: {'status': 'offline'}),
    );
    await tester.pump();

    expect(find.text('伺服器未連線'), findsOneWidget);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('conversation-composer')))
          .enabled,
      isFalse,
    );
  });
}

class _FakeShareImportService implements MikuShareImportService {
  final StreamController<SharedContent> _controller =
      StreamController<SharedContent>.broadcast();

  @override
  bool get isSupported => true;

  @override
  Stream<SharedContent> get imports => _controller.stream;

  void add(SharedContent content) => _controller.add(content);

  Future<void> close() => _controller.close();
}

double _contrastRatio(Color first, Color second) {
  final lighter =
      first.computeLuminance() > second.computeLuminance() ? first : second;
  final darker = lighter == first ? second : first;
  return (lighter.computeLuminance() + 0.05) /
      (darker.computeLuminance() + 0.05);
}
