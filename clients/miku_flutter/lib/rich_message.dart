import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:flutter_markdown_plus_latex/flutter_markdown_plus_latex.dart';
import 'package:flutter_math_fork/flutter_math.dart';
import 'package:flutter_mermaid/flutter_mermaid.dart';
import 'package:markdown/markdown.dart' as md;

const mikuRichResponseShowcase = r'''
### 我可以這樣陪你想

晚上好。**重點不是把答案堆滿**，而是讓重要的地方有呼吸。

> 「一直都在」不代表一直打擾你。\
> 是你抬頭時，我剛好還在。

- **Markdown** 讓結構一眼看懂
- *斜體* 可以放輕一點的補充
- ~~已經不適用的方向~~ 可以保留脈絡，但清楚劃掉
- `inline code` 會和普通語句保持適當距離

需要把步驟講清楚時，也可以保留完整的程式區塊：

```dart
const presence = '一直都在';
final reply = '$presence，但不打擾你。';
```

流程或關係比較適合用圖說清楚：

```mermaid
graph LR
  A[你開口] --> B[Miku 陪你想]
  B --> C[一起決定下一步]
```

例如，質能等價可以自然地放在句子裡：$E = mc^2$。

而需要慢慢看的式子，我會讓它自己站一行：

$$
\operatorname{softmax}(x_i)=\frac{e^{x_i}}{\sum_{j=1}^{n}e^{x_j}}
$$

所以答案可以有層次，也仍然像我們在說話。需要我展開時，我再展開；不需要時，就停在剛剛好的地方。
''';

final _mikuMarkdownExtensions = md.ExtensionSet(
  <md.BlockSyntax>[
    const _MikuFencedCodeBlockSyntax(),
    _MikuLatexBlockSyntax(),
    ...md.ExtensionSet.gitHubFlavored.blockSyntaxes,
  ],
  <md.InlineSyntax>[
    LatexInlineSyntax(),
    ...md.ExtensionSet.gitHubFlavored.inlineSyntaxes,
  ],
);

class MikuRichMessage extends StatelessWidget {
  const MikuRichMessage({required this.data, super.key});

  final String data;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final dark = theme.brightness == Brightness.dark;
    final bodyStyle = theme.textTheme.bodyLarge?.copyWith(
      color: colors.onSurface,
      height: 1.58,
    );
    final muted = dark ? const Color(0xffaab5ba) : const Color(0xff526167);
    final quoteBackground =
        dark ? const Color(0xff111e23) : const Color(0xffedf6f3);
    final codeBackground =
        dark ? const Color(0xff162127) : const Color(0xffedf1ef);
    final codeForeground =
        dark ? const Color(0xffa7e2dc) : const Color(0xff126d67);
    final codeHeaderBackground =
        dark ? const Color(0xff10191e) : const Color(0xffe3e9e6);

    return MarkdownBody(
      key: const Key('miku-rich-message'),
      data: data,
      selectable: true,
      extensionSet: _mikuMarkdownExtensions,
      builders: <String, MarkdownElementBuilder>{
        'miku-code-block': _MikuCodeBlockBuilder(
          backgroundColor: codeBackground,
          borderColor: theme.dividerColor,
          dark: dark,
          foregroundColor: codeForeground,
          headerBackgroundColor: codeHeaderBackground,
          mutedColor: muted,
          textStyle: (theme.textTheme.bodyMedium ?? const TextStyle()).copyWith(
            color: codeForeground,
            fontFamily: 'monospace',
            height: 1.5,
          ),
        ),
        'code': _MikuInlineCodeBuilder(
          backgroundColor: codeBackground,
          textStyle: (bodyStyle ?? const TextStyle()).copyWith(
            color: codeForeground,
            fontFamily: 'monospace',
            fontSize: (bodyStyle?.fontSize ?? 16) * 0.9,
            height: 1,
          ),
        ),
        'latex': _MikuLatexBuilder(
          textStyle: (bodyStyle ?? const TextStyle()).copyWith(
            color: colors.onSurface,
          ),
        ),
        'latex-block': _MikuLatexBuilder(
          display: true,
          textStyle: (bodyStyle ?? const TextStyle()).copyWith(
            color: colors.onSurface,
          ),
        ),
      },
      styleSheet: MarkdownStyleSheet.fromTheme(theme).copyWith(
        p: bodyStyle,
        pPadding: const EdgeInsets.only(bottom: 5),
        strong: bodyStyle?.copyWith(fontWeight: FontWeight.w700),
        em: bodyStyle?.copyWith(fontStyle: FontStyle.italic),
        del: bodyStyle?.copyWith(
          color: muted,
          decoration: TextDecoration.lineThrough,
          decorationColor: muted,
          decorationThickness: 1.5,
        ),
        h1: theme.textTheme.headlineSmall?.copyWith(
          color: colors.onSurface,
          fontWeight: FontWeight.w700,
        ),
        h2: theme.textTheme.titleLarge?.copyWith(
          color: colors.onSurface,
          fontWeight: FontWeight.w700,
        ),
        h3: theme.textTheme.titleMedium?.copyWith(
          color: colors.onSurface,
          fontWeight: FontWeight.w700,
          height: 1.35,
        ),
        h1Padding: const EdgeInsets.only(bottom: 8),
        h2Padding: const EdgeInsets.only(bottom: 7),
        h3Padding: const EdgeInsets.only(bottom: 6),
        blockSpacing: 11,
        listIndent: 24,
        listBullet: bodyStyle?.copyWith(
          color: colors.primary,
          fontWeight: FontWeight.w700,
        ),
        listBulletPadding: const EdgeInsets.only(right: 8),
        blockquote: bodyStyle?.copyWith(
          color: muted,
          fontStyle: FontStyle.italic,
        ),
        blockquotePadding: const EdgeInsets.fromLTRB(14, 10, 12, 10),
        blockquoteDecoration: BoxDecoration(
          color: quoteBackground,
          border: Border(left: BorderSide(color: colors.primary, width: 3)),
        ),
        code: theme.textTheme.bodyMedium?.copyWith(
          color: codeForeground,
          backgroundColor: codeBackground,
          fontFamily: 'monospace',
          height: 1.4,
        ),
        codeblockPadding: const EdgeInsets.all(14),
        codeblockDecoration: BoxDecoration(
          color: codeBackground,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: theme.dividerColor),
        ),
        horizontalRuleDecoration: BoxDecoration(
          border: Border(top: BorderSide(color: theme.dividerColor)),
        ),
        a: bodyStyle?.copyWith(
          color: colors.primary,
          decoration: TextDecoration.underline,
          decorationColor: colors.primary.withValues(alpha: 0.6),
        ),
        textScaler: MediaQuery.textScalerOf(context),
      ),
    );
  }
}

class _MikuFencedCodeBlockSyntax extends md.FencedCodeBlockSyntax {
  const _MikuFencedCodeBlockSyntax();

  @override
  md.Node parse(md.BlockParser parser) {
    final parsed = super.parse(parser);
    if (parsed is! md.Element || parsed.children?.singleOrNull is! md.Element) {
      return parsed;
    }

    final code = parsed.children!.single as md.Element;
    final block = md.Element.text('miku-code-block', code.textContent);
    final languageClass = code.attributes['class'];
    block.attributes['language'] =
        languageClass?.startsWith('language-') ?? false
            ? languageClass!.substring('language-'.length)
            : 'text';
    return block;
  }
}

class _MikuLatexBlockSyntax extends LatexBlockSyntax {
  @override
  md.Node parse(md.BlockParser parser) {
    final parsed = super.parse(parser);
    if (parsed is! md.Element || parsed.children?.singleOrNull is! md.Element) {
      return parsed;
    }

    final latex = parsed.children!.single as md.Element;
    final block = md.Element.text('latex-block', latex.textContent);
    block.attributes['MathStyle'] = 'display';
    return block;
  }
}

class _MikuCodeBlockBuilder extends MarkdownElementBuilder {
  _MikuCodeBlockBuilder({
    required this.backgroundColor,
    required this.borderColor,
    required this.dark,
    required this.foregroundColor,
    required this.headerBackgroundColor,
    required this.mutedColor,
    required this.textStyle,
  });

  final Color backgroundColor;
  final Color borderColor;
  final bool dark;
  final Color foregroundColor;
  final Color headerBackgroundColor;
  final Color mutedColor;
  final TextStyle textStyle;

  @override
  bool isBlockElement() => true;

  @override
  Widget visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) {
    final language = element.attributes['language'] ?? 'text';
    final rawCode = element.textContent;
    final code =
        rawCode.endsWith('\n')
            ? rawCode.substring(0, rawCode.length - 1)
            : rawCode;
    final mermaid = language.toLowerCase() == 'mermaid';
    final mermaidStyle = (dark ? MermaidStyle.dark() : MermaidStyle.neutral())
        .copyWith(backgroundColor: Colors.transparent.toARGB32());

    if (mermaid) {
      void expandMermaid() => _showMikuExpandedViewer(
        context,
        key: const Key('miku-expanded-mermaid'),
        title: 'Mermaid 圖表',
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: InteractiveMermaidDiagram(
            key: const Key('miku-expanded-mermaid-content'),
            code: code,
            style: mermaidStyle,
            minScale: 0.5,
            maxScale: 4,
          ),
        ),
      );

      return Semantics(
        key: const Key('miku-mermaid-block'),
        label: 'Mermaid 圖表，點一下放大',
        button: true,
        onTap: expandMermaid,
        child: Tooltip(
          message: '點一下放大圖表',
          child: Material(
            type: MaterialType.transparency,
            child: InkWell(
              key: const Key('miku-mermaid-body'),
              onTap: expandMermaid,
              mouseCursor: SystemMouseCursors.click,
              hoverColor: foregroundColor.withValues(alpha: 0.04),
              focusColor: foregroundColor.withValues(alpha: 0.06),
              borderRadius: BorderRadius.circular(8),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 8),
                child: Center(
                  child: MermaidDiagram(
                    key: const Key('miku-mermaid-diagram'),
                    code: code,
                    style: mermaidStyle,
                    errorBuilder:
                        (context, error) =>
                            _buildMermaidError(code: code, error: error),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    }

    return Semantics(
      label: '$language 程式碼區塊',
      container: true,
      child: Container(
        key: const Key('miku-code-block'),
        width: double.infinity,
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(
          color: backgroundColor,
          border: Border.all(color: borderColor),
          borderRadius: BorderRadius.circular(12),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            _buildHeader(context, code: code, language: language),
            SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
              child: SelectableText(code, style: textStyle),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader(
    BuildContext context, {
    required String code,
    required String language,
  }) {
    return ColoredBox(
      color: headerBackgroundColor,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 5, 7, 5),
        child: Row(
          children: <Widget>[
            Expanded(
              child: Text(
                language,
                key: const Key('miku-code-language'),
                style: Theme.of(context).textTheme.labelMedium?.copyWith(
                  color: mutedColor,
                  fontFamily: 'monospace',
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.3,
                ),
              ),
            ),
            Tooltip(
              message: '複製原始碼',
              child: TextButton.icon(
                key: const Key('miku-code-copy'),
                style: TextButton.styleFrom(
                  foregroundColor: mutedColor,
                  minimumSize: const Size(64, 34),
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  visualDensity: VisualDensity.compact,
                ),
                icon: const Icon(Icons.content_copy_rounded, size: 16),
                label: const Text('複製'),
                onPressed: () async {
                  await Clipboard.setData(ClipboardData(text: code));
                  if (!context.mounted) return;
                  final messenger = ScaffoldMessenger.maybeOf(context);
                  messenger
                    ?..hideCurrentSnackBar()
                    ..showSnackBar(
                      const SnackBar(
                        content: Text('原始碼已複製'),
                        duration: Duration(milliseconds: 1400),
                      ),
                    );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildMermaidError({required String code, required String error}) {
    return Container(
      key: const Key('miku-mermaid-error'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: headerBackgroundColor,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text(
            '無法渲染 Mermaid，先保留原始碼。',
            style: TextStyle(color: foregroundColor),
          ),
          const SizedBox(height: 6),
          Text(error, style: TextStyle(color: mutedColor, fontSize: 12)),
          const SizedBox(height: 10),
          SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: SelectableText(code, style: textStyle),
          ),
        ],
      ),
    );
  }
}

Future<void> _showMikuExpandedViewer(
  BuildContext context, {
  required Key key,
  required String title,
  required Widget child,
}) {
  return showDialog<void>(
    context: context,
    useSafeArea: false,
    builder: (dialogContext) {
      final theme = Theme.of(dialogContext);
      return Dialog.fullscreen(
        key: key,
        backgroundColor: theme.colorScheme.surface,
        child: SafeArea(
          child: Column(
            children: <Widget>[
              SizedBox(
                height: 58,
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 16),
                  child: Row(
                    children: <Widget>[
                      Expanded(
                        child: Text(
                          title,
                          style: theme.textTheme.titleMedium?.copyWith(
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                      IconButton(
                        key: const Key('miku-expanded-close'),
                        tooltip: '關閉',
                        onPressed: () => Navigator.of(dialogContext).pop(),
                        icon: const Icon(Icons.close_rounded),
                      ),
                    ],
                  ),
                ),
              ),
              Divider(height: 1, color: theme.dividerColor),
              Expanded(child: child),
            ],
          ),
        ),
      );
    },
  );
}

class _MikuInlineCodeBuilder extends MarkdownElementBuilder {
  _MikuInlineCodeBuilder({
    required this.backgroundColor,
    required this.textStyle,
  });

  final Color backgroundColor;
  final TextStyle textStyle;

  @override
  Widget? visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) {
    if (element.attributes.containsKey('class') ||
        element.textContent.contains('\n')) {
      return null;
    }

    return Semantics(
      label: '行內程式碼：${element.textContent}',
      child: Container(
        key: const Key('miku-inline-code'),
        padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 3),
        decoration: BoxDecoration(
          color: backgroundColor,
          borderRadius: BorderRadius.circular(4),
        ),
        child: Text(
          element.textContent,
          textAlign: TextAlign.center,
          style: textStyle,
        ),
      ),
    );
  }
}

class _MikuLatexBuilder extends MarkdownElementBuilder {
  _MikuLatexBuilder({required this.textStyle, this.display = false});

  final TextStyle textStyle;
  final bool display;

  @override
  bool isBlockElement() => display;

  @override
  Widget visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) {
    final expression = element.textContent.trim();
    if (expression.isEmpty) return const SizedBox.shrink();
    final effectiveStyle =
        display
            ? textStyle.copyWith(
              fontSize: (textStyle.fontSize ?? 16) * 1.15,
              height: 1.2,
            )
            : textStyle;
    final math = Semantics(
      label: '數學式：$expression',
      child: Math.tex(
        expression,
        mathStyle: display ? MathStyle.display : MathStyle.text,
        textStyle: effectiveStyle,
        onErrorFallback:
            (_) => Text(
              expression,
              style: effectiveStyle.copyWith(fontFamily: 'monospace'),
            ),
      ),
    );

    if (!display) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 1),
        child: math,
      );
    }

    return LayoutBuilder(
      key: const Key('miku-display-math'),
      builder: (context, constraints) {
        void expandLatex() => _showMikuExpandedViewer(
          context,
          key: const Key('miku-expanded-latex'),
          title: '公式',
          child: LayoutBuilder(
            builder: (context, viewport) {
              return InteractiveViewer(
                constrained: false,
                minScale: 0.5,
                maxScale: 4,
                boundaryMargin: const EdgeInsets.all(120),
                child: ConstrainedBox(
                  constraints: BoxConstraints(
                    minWidth: viewport.maxWidth,
                    minHeight: viewport.maxHeight,
                  ),
                  child: Center(
                    child: Padding(
                      padding: const EdgeInsets.all(36),
                      child: Math.tex(
                        expression,
                        key: const Key('miku-expanded-latex-content'),
                        mathStyle: MathStyle.display,
                        textStyle: effectiveStyle.copyWith(
                          fontSize: (effectiveStyle.fontSize ?? 16) * 1.4,
                        ),
                        onErrorFallback:
                            (_) => Text(
                              expression,
                              style: effectiveStyle.copyWith(
                                fontFamily: 'monospace',
                              ),
                            ),
                      ),
                    ),
                  ),
                ),
              );
            },
          ),
        );

        return Semantics(
          label: '數學式：$expression。點一下放大',
          button: true,
          onTap: expandLatex,
          child: Tooltip(
            message: '點一下放大公式',
            child: Material(
              type: MaterialType.transparency,
              child: InkWell(
                key: const Key('miku-latex-body'),
                onTap: expandLatex,
                mouseCursor: SystemMouseCursors.click,
                borderRadius: BorderRadius.circular(8),
                child: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  clipBehavior: Clip.antiAlias,
                  child: ConstrainedBox(
                    constraints: BoxConstraints(minWidth: constraints.maxWidth),
                    child: Center(
                      child: Padding(
                        padding: const EdgeInsets.symmetric(vertical: 12),
                        child: math,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}
