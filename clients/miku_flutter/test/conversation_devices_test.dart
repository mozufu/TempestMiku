import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/session_client_stub.dart';

void main() {
  testWidgets(
    'unknown device identity disables revocation on every active device',
    (tester) async {
      tester.view.physicalSize = const Size(375, 812);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      final client = _UnknownIdentityClient();
      await tester.pumpWidget(
        TempestMikuApp(client: client, themeMode: ThemeMode.light),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));

      await tester.tap(find.byKey(const Key('open-left-drawer')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('drawer-settings')));
      await tester.pumpAndSettle();

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

      // Both scripted devices are active, yet no row offers revocation.
      expect(
        find.byKey(const Key('auth-device-device-current')),
        findsOneWidget,
      );
      expect(
        find.byKey(const Key('auth-device-device-browser')),
        findsOneWidget,
      );
      expect(find.byIcon(Icons.link_off_rounded), findsNothing);
      expect(find.byKey(const Key('current-auth-device')), findsNothing);
      expect(find.text('暫不可撤銷'), findsNWidgets(2));
      expect(find.text('暫時無法確認目前這台裝置，為了安全先停用撤銷。'), findsOneWidget);
    },
  );

  testWidgets('known device identity keeps revocation for other devices', (
    tester,
  ) async {
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
    await tester.tap(find.byKey(const Key('drawer-settings')));
    await tester.pumpAndSettle();

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

    expect(find.byKey(const Key('current-auth-device')), findsOneWidget);
    expect(find.byTooltip('撤銷 Laptop browser'), findsOneWidget);
    expect(find.text('暫不可撤銷'), findsNothing);
  });
}

final class _UnknownIdentityClient extends ScriptedMikuClient {
  @override
  Future<String?> currentAuthDeviceId() async {
    throw StateError('installation-local device hint unavailable');
  }
}
