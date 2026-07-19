import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_math_fork/flutter_math.dart';
import 'package:flutter_mermaid/flutter_mermaid.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/rich_message.dart';

void main() {
  testWidgets('renders rich Markdown, code, Mermaid, and LaTeX', (
    tester,
  ) async {
    String? copiedText;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'Clipboard.setData') {
            copiedText =
                (call.arguments as Map<Object?, Object?>)['text'] as String;
          }
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
    );
    tester.view.physicalSize = const Size(375, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData.light(useMaterial3: true),
        home: const Scaffold(
          body: SingleChildScrollView(
            padding: EdgeInsets.all(16),
            child: MikuRichMessage(data: mikuRichResponseShowcase),
          ),
        ),
      ),
    );
    await tester.pump();

    expect(find.byKey(const Key('miku-rich-message')), findsOneWidget);
    expect(find.text('我可以這樣陪你想'), findsOneWidget);
    expect(find.textContaining('一直都在'), findsWidgets);
    expect(find.textContaining("const presence = '一直都在';"), findsOneWidget);
    expect(find.textContaining('final reply ='), findsOneWidget);
    expect(find.byKey(const Key('miku-code-block')), findsOneWidget);
    expect(find.byKey(const Key('miku-code-language')), findsOneWidget);
    expect(find.text('dart'), findsOneWidget);
    expect(find.byKey(const Key('miku-code-copy')), findsOneWidget);
    expect(find.byKey(const Key('miku-mermaid-block')), findsOneWidget);
    expect(find.byKey(const Key('miku-mermaid-body')), findsOneWidget);
    expect(find.byKey(const Key('miku-mermaid-language')), findsNothing);
    expect(find.text('mermaid'), findsNothing);
    expect(find.byKey(const Key('miku-mermaid-copy')), findsNothing);
    expect(find.byKey(const Key('miku-mermaid-expand')), findsNothing);
    expect(find.byType(MermaidDiagram), findsOneWidget);
    expect(find.byType(Math), findsNWidgets(2));
    expect(find.byKey(const Key('miku-inline-code')), findsOneWidget);
    expect(find.byKey(const Key('miku-display-math')), findsOneWidget);
    expect(find.byKey(const Key('miku-latex-body')), findsOneWidget);
    expect(find.byKey(const Key('miku-latex-expand')), findsNothing);
    expect(_hasStrikethrough(tester), isTrue);

    expect(
      tester.getSize(find.byKey(const Key('miku-code-block'))).width,
      closeTo(
        tester.getSize(find.byKey(const Key('miku-rich-message'))).width,
        0.5,
      ),
    );
    expect(
      tester.getSize(find.byKey(const Key('miku-mermaid-block'))).width,
      closeTo(
        tester.getSize(find.byKey(const Key('miku-rich-message'))).width,
        0.5,
      ),
    );
    await tester.tap(find.byKey(const Key('miku-code-copy')));
    await tester.pump();
    expect(copiedText, contains("const presence = '一直都在';"));
    expect(find.text('原始碼已複製'), findsOneWidget);

    final mermaidBody = find.byKey(const Key('miku-mermaid-body'));
    await tester.ensureVisible(mermaidBody);
    await tester.pump();
    await tester.tap(mermaidBody);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('miku-expanded-mermaid')), findsOneWidget);
    expect(
      find.byKey(const Key('miku-expanded-mermaid-content')),
      findsOneWidget,
    );
    expect(find.byType(InteractiveMermaidDiagram), findsOneWidget);
    await tester.tap(find.byKey(const Key('miku-expanded-close')));
    await tester.pumpAndSettle();

    final latexBody = find.byKey(const Key('miku-latex-body'));
    await tester.ensureVisible(latexBody);
    await tester.pump();
    await tester.tap(latexBody);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('miku-expanded-latex')), findsOneWidget);
    expect(
      find.byKey(const Key('miku-expanded-latex-content')),
      findsOneWidget,
    );
    await tester.tap(find.byKey(const Key('miku-expanded-close')));
    await tester.pumpAndSettle();

    final inlineCode = tester.widget<Container>(
      find.byKey(const Key('miku-inline-code')),
    );
    expect(
      inlineCode.padding,
      const EdgeInsets.symmetric(horizontal: 5, vertical: 3),
    );
    expect(
      tester
          .widget<Text>(
            find.descendant(
              of: find.byKey(const Key('miku-inline-code')),
              matching: find.byType(Text),
            ),
          )
          .textAlign,
      TextAlign.center,
    );

    final displayMathFinder = find.descendant(
      of: find.byKey(const Key('miku-display-math')),
      matching: find.byType(Math),
    );
    final mathWidgets = tester.widgetList<Math>(find.byType(Math)).toList();
    final displayMath = tester.widget<Math>(displayMathFinder);
    final inlineMath = mathWidgets.firstWhere((math) => math != displayMath);
    expect(
      displayMath.textStyle!.fontSize,
      greaterThan(inlineMath.textStyle!.fontSize!),
    );
    expect(
      tester.getCenter(displayMathFinder).dx,
      closeTo(
        tester.getCenter(find.byKey(const Key('miku-display-math'))).dx,
        0.5,
      ),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('keeps Mermaid source visible when parsing fails', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData.dark(useMaterial3: true),
        home: const Scaffold(
          body: MikuRichMessage(
            data: r'''```mermaid
this is not a diagram
```''',
          ),
        ),
      ),
    );
    await tester.pump();

    expect(find.byKey(const Key('miku-mermaid-error')), findsOneWidget);
    expect(find.textContaining('無法渲染 Mermaid'), findsOneWidget);
    expect(find.textContaining('this is not a diagram'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}

bool _hasStrikethrough(WidgetTester tester) {
  for (final richText in tester.widgetList<RichText>(find.byType(RichText))) {
    if (_spanHasStrikethrough(richText.text)) return true;
  }
  for (final selectable in tester.widgetList<SelectableText>(
    find.byType(SelectableText),
  )) {
    final span = selectable.textSpan;
    if (span != null && _spanHasStrikethrough(span)) return true;
  }
  return false;
}

bool _spanHasStrikethrough(InlineSpan span) {
  if (span.style?.decoration?.contains(TextDecoration.lineThrough) ?? false) {
    return true;
  }
  if (span is TextSpan) {
    for (final child in span.children ?? const <InlineSpan>[]) {
      if (_spanHasStrikethrough(child)) return true;
    }
  }
  return false;
}
