import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';

void main() {
  Widget appFor(ScriptedMikuClient client) => TempestMikuApp(
    client: client,
    themeMode: ThemeMode.light,
    now: () => DateTime(2026, 7, 19, 20),
  );

  Future<void> loadApp(WidgetTester tester, ScriptedMikuClient client) async {
    await tester.pumpWidget(appFor(client));
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

  testWidgets('expands server project and history and switches sessions', (
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
    expect(find.byKey(const Key('drawer-project-content')), findsOneWidget);
    expect(find.text('Scripted project status'), findsOneWidget);
    expect(
      find.textContaining('Continue from latest session result'),
      findsOneWidget,
    );

    await tester.tap(find.byKey(const Key('drawer-history')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('drawer-history-content')), findsOneWidget);
    expect(find.byKey(Key('history-session-${first.id}')), findsOneWidget);
    expect(find.byKey(Key('history-session-${second.id}')), findsOneWidget);
    expect(
      find.byKey(const Key('drawer-project-content')),
      findsOneWidget,
      reason: 'Project and History should stay independently expanded.',
    );

    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.byKey(Key('history-session-${first.id}')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(Key('history-session-${first.id}')));
    await tester.pumpAndSettle();

    final scaffold = tester.state<ScaffoldState>(find.byType(Scaffold));
    expect(scaffold.isDrawerOpen, isFalse);
    expect(find.text('第一段對話'), findsOneWidget);
    expect((await client.createOrReuseSession()).id, first.id);
  });

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

  testWidgets('shows project catalog empty and retryable error states', (
    tester,
  ) async {
    final emptyClient = ScriptedMikuClient(projectCatalogEmpty: true);
    await loadApp(tester, emptyClient);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    expect(find.text('尚未連結任何 Project。'), findsOneWidget);

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

    client.completePausedTurn();
    await tester.pump();

    expect(find.text('伺服器已連線'), findsOneWidget);
    expect(find.text('Miku heard: 慢慢回答'), findsOneWidget);
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

double _contrastRatio(Color first, Color second) {
  final lighter =
      first.computeLuminance() > second.computeLuminance() ? first : second;
  final darker = lighter == first ? second : first;
  return (lighter.computeLuminance() + 0.05) /
      (darker.computeLuminance() + 0.05);
}
