part of 'widget_test.dart';

void _registerSessionShellTests() {
  testWidgets('compact mobile shell keeps controls in the drawer at 390px', (
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
    expect(find.byType(NavigationBar), findsNothing);
    expect(find.byType(NavigationRail), findsNothing);
    expect(find.byTooltip('Open menu'), findsOneWidget);
    expect(find.text('Sessions'), findsNothing);
    expect(find.text('Drive'), findsNothing);
    await tester.tap(find.byTooltip('Open menu'));
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Sessions'), findsWidgets);
    expect(find.text('Context'), findsOneWidget);
    expect(find.text('Settings'), findsOneWidget);
    expect(find.text('Personal'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('desktop shell remains chat-only until its drawer is opened', (
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

    expect(find.byType(NavigationRail), findsNothing);
    expect(find.byType(NavigationBar), findsNothing);
    expect(find.text('Sessions'), findsNothing);
    expect(find.text('Context'), findsNothing);
    expect(find.text('Project status'), findsNothing);
    expect(find.byType(TextField), findsOneWidget);
    await tester.tap(find.byTooltip('Open menu'));
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Sessions'), findsWidgets);
    expect(find.text('Context'), findsOneWidget);
    expect(find.text('Settings'), findsOneWidget);
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

    expect(find.byType(NavigationBar), findsNothing);
    expect(find.byType(NavigationRail), findsNothing);
    expect(find.byTooltip('Open menu'), findsOneWidget);
    expect(find.text('Sessions'), findsNothing);
    expect(find.text('Drive'), findsNothing);
    expect(find.bySemanticsLabel('Send message'), findsOneWidget);

    await tester.enterText(find.byType(EditableText), 'code artifact://0');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.byKey(const ValueKey('resource:artifact://0')), findsOneWidget);
  });
}

void _registerSessionAndModeTests() {
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
    await _scrollDrawerUntilVisible(tester, find.text('Language'));
    await tester.tap(find.text('Language'));
    await tester.pump(const Duration(milliseconds: 100));
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

      expect(find.text('Miku'), findsOneWidget);
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

      await _tapDrawerAction(tester, 'Promote Session');

      await _openContext(tester);
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

      await _startNewSessionFromDrawer(tester);
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

    await _tapDrawerAction(tester, 'Mode settings');
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

    await _tapDrawerAction(tester, 'Mode settings');
    await tester.pump(const Duration(milliseconds: 350));

    await tester.ensureVisible(find.text('Lock Serious'));
    await tester.pump();
    expect(find.text('Lock Serious'), findsOneWidget);
    await tester.tap(find.text('Lock Serious'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.lockedModes, contains('serious_engineer'));
    await _tapDrawerAction(tester, 'Mode settings');
    await tester.pump(const Duration(milliseconds: 350));
    await tester.ensureVisible(find.text('Unlock Serious'));
    await tester.pump();
    expect(find.text('Unlock Serious'), findsOneWidget);
  });
}
