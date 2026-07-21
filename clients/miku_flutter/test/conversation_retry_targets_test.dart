import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';

void main() {
  Future<void> loadApp(WidgetTester tester, ScriptedMikuClient client) async {
    await tester.pumpWidget(
      TempestMikuApp(client: client, themeMode: ThemeMode.light),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
  }

  testWidgets('resources inspector retry re-previews the failed URI', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    const target = 'history://scripted-actor/output';
    final client = _FlakyPreviewClient(failUri: target);
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-resources')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('resource-inspector')), findsOneWidget);
    final directoryListsBeforeFailure = client.listedUris.length;

    await tester.tap(find.byKey(const Key('open-exact-resource-uri')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('exact-resource-uri-input')),
      target,
    );
    await tester.tap(find.byKey(const Key('confirm-exact-resource-uri')));
    await tester.pumpAndSettle();

    expect(find.text('這個資源目前無法預覽。'), findsOneWidget);
    expect(client.previewedUris, [target]);

    await tester.tap(find.widgetWithText(TextButton, '重試'));
    await tester.pumpAndSettle();

    // Retry re-previews the same URI instead of reloading the directory.
    expect(client.previewedUris, [target, target]);
    expect(client.listedUris.length, directoryListsBeforeFailure);
    expect(find.byKey(const Key('resource-preview-content')), findsOneWidget);
    expect(find.textContaining('Preview for $target'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('project browser retry re-previews the failed file entry', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    const readme = 'project://tempestmiku/linked-folders/tempestmiku/README.md';
    final client = _FlakyResolveClient(failUri: readme);
    final session = await client.createSession();
    await client.setSessionScope(session.id, 'project:tempestmiku');
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-project')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('project-page-content')), findsOneWidget);
    final directoryListsBeforeFailure = client.listedUris.length;

    await tester.ensureVisible(
      find.byKey(const Key('project-resource-$readme')),
    );
    await tester.tap(find.byKey(const Key('project-resource-$readme')));
    await tester.pumpAndSettle();

    expect(find.text('Project 暫時讀不到，請再試一次。'), findsOneWidget);
    expect(find.byKey(const Key('project-file-content')), findsNothing);
    expect(client.resolvedUris, [readme]);

    await tester.tap(find.byTooltip('重試'));
    await tester.pumpAndSettle();

    // Retry re-resolves the same file instead of reloading the folder.
    expect(client.resolvedUris, [readme, readme]);
    expect(client.listedUris.length, directoryListsBeforeFailure);
    expect(find.byKey(const Key('project-file-title')), findsOneWidget);
    expect(find.byKey(const Key('project-file-content')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('drive 檢視方式 chips preview the virtual directory', (tester) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final client = _FlakyPreviewClient(failUri: null);
    await loadApp(tester, client);
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-drive')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('drive-page-content')), findsOneWidget);

    const virtualDirUri = 'drive://recent';
    final chip = find.byKey(const Key('drive-virtual-dir-$virtualDirUri'));
    await tester.ensureVisible(chip);
    expect(tester.widget<ActionChip>(chip).onPressed, isNotNull);
    await tester.tap(chip);
    await tester.pumpAndSettle();

    expect(client.previewedUris, [virtualDirUri]);
    expect(find.byKey(const Key('drive-preview-title')), findsOneWidget);
    expect(find.byKey(const Key('drive-preview-content')), findsOneWidget);
    expect(find.textContaining('Local citation corpus'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}

/// Records preview/list calls; fails `previewResource` for [failUri] once.
final class _FlakyPreviewClient extends ScriptedMikuClient {
  _FlakyPreviewClient({required this.failUri});

  final String? failUri;
  final List<String> previewedUris = [];
  final List<String> listedUris = [];
  int _failuresRemaining = 1;

  @override
  Future<ResourcePreview> previewResource(String sessionId, String uri) async {
    previewedUris.add(uri);
    if (uri == failUri && _failuresRemaining > 0) {
      _failuresRemaining--;
      throw StateError('scripted preview failure');
    }
    return super.previewResource(sessionId, uri);
  }

  @override
  Future<List<MikuResourceEntry>> listResources(
    String sessionId,
    String uri,
  ) async {
    listedUris.add(uri);
    return super.listResources(sessionId, uri);
  }
}

/// Records resolve/list calls; fails `resolveResource` for [failUri] once.
final class _FlakyResolveClient extends ScriptedMikuClient {
  _FlakyResolveClient({required this.failUri});

  final String failUri;
  final List<String> resolvedUris = [];
  final List<String> listedUris = [];
  int _failuresRemaining = 1;

  @override
  Future<ResourcePreview> resolveResource(
    String sessionId,
    String uri, {
    String? selector,
  }) async {
    if (uri == failUri) {
      resolvedUris.add(uri);
      if (_failuresRemaining > 0) {
        _failuresRemaining--;
        throw StateError('scripted resolve failure');
      }
    }
    return super.resolveResource(sessionId, uri, selector: selector);
  }

  @override
  Future<List<MikuResourceEntry>> listResources(
    String sessionId,
    String uri,
  ) async {
    listedUris.add(uri);
    return super.listResources(sessionId, uri);
  }
}
