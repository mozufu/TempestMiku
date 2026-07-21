import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/session_client_stub.dart';

void main() {
  testWidgets(
    'resource inspector replaces compact preview then appends bounded pages',
    (tester) async {
      tester.view.physicalSize = const Size(375, 812);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      final client = ScriptedMikuClient();
      await tester.pumpWidget(
        TempestMikuApp(client: client, themeMode: ThemeMode.light),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      await tester.tap(find.byKey(const Key('open-left-drawer')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('drawer-resources')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('resource-entry-artifact://')));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const Key('resource-entry-artifact://scripted-report')),
      );
      await tester.pumpAndSettle();

      expect(
        find.text('Preview for artifact://scripted-report (compact)'),
        findsOneWidget,
      );
      final firstLoad = find.byKey(const Key('resource-load-more'));
      expect(firstLoad, findsOneWidget);
      expect(tester.getSize(firstLoad).height, greaterThanOrEqualTo(44));

      await tester.tap(firstLoad);
      await tester.pumpAndSettle();
      expect(client.resolvedResourceSelectors, ['1-200']);
      expect(
        find.text('Preview for artifact://scripted-report (compact)'),
        findsNothing,
      );
      expect(find.text('Resolved lines 1-200'), findsOneWidget);
      expect(find.byKey(const Key('resource-load-more')), findsOneWidget);

      client.failResourceResolve = true;
      await tester.tap(find.byKey(const Key('resource-load-more')));
      await tester.pumpAndSettle();
      expect(client.resolvedResourceSelectors, ['1-200', '201-400']);
      expect(find.text('Resolved lines 1-200'), findsOneWidget);
      expect(find.byKey(const Key('resource-load-more-error')), findsOneWidget);
      expect(
        tester
            .getSize(find.byKey(const Key('resource-load-more-retry')))
            .height,
        greaterThanOrEqualTo(44),
      );

      client.failResourceResolve = false;
      await tester.tap(find.byKey(const Key('resource-load-more-retry')));
      await tester.pumpAndSettle();
      expect(client.resolvedResourceSelectors, ['1-200', '201-400', '201-400']);
      expect(
        find.text('Resolved lines 1-200\nResolved lines 201-400'),
        findsOneWidget,
      );
      expect(find.byKey(const Key('resource-load-more')), findsNothing);
      expect(find.byKey(const Key('resource-load-more-error')), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets('system back walks up the resource inspector path', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(
      TempestMikuApp(client: client, themeMode: ThemeMode.light),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-resources')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('resource-entry-artifact://')));
    await tester.pumpAndSettle();

    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();

    expect(find.byKey(const Key('resource-inspector')), findsOneWidget);
    expect(find.byKey(const Key('resource-entry-artifact://')), findsOneWidget);
    expect(
      find.byKey(const Key('resource-entry-artifact://scripted-report')),
      findsNothing,
    );
  });
}
