part of 'conversation_screen.dart';

class _ResourceLocation {
  const _ResourceLocation({required this.uri, required this.label});

  final String uri;
  final String label;
}

class _ResourceInspectorSheet extends StatefulWidget {
  const _ResourceInspectorSheet({
    required this.client,
    required this.sessionId,
  });

  final MikuSessionClient client;
  final String sessionId;

  @override
  State<_ResourceInspectorSheet> createState() =>
      _ResourceInspectorSheetState();
}

class _ResourceInspectorSheetState extends State<_ResourceInspectorSheet> {
  final List<_ResourceLocation> _path =
      const [_ResourceLocation(uri: '', label: '全部')].toList();
  List<MikuResourceEntry>? _entries;
  ResourcePreview? _preview;
  _ResourceLocation? _failedLocation;
  String? _failedPreviewUri;
  final Set<String> _registeredSchemes = {};
  bool _loading = false;
  bool _loadingResourcePage = false;
  int _nextResourceLine = 1;
  String? _error;
  String? _resourcePageError;

  bool get _busy => _loading || _loadingResourcePage;

  @override
  void initState() {
    super.initState();
    unawaited(_load(_path.first, push: false));
  }

  Future<void> _load(_ResourceLocation location, {required bool push}) async {
    if (_busy) return;
    setState(() {
      _loading = true;
      _error = null;
      _preview = null;
      _resourcePageError = null;
      _nextResourceLine = 1;
      _failedLocation = null;
      _failedPreviewUri = null;
    });
    try {
      final entries = await widget.client.listResources(
        widget.sessionId,
        location.uri,
      );
      if (!mounted) return;
      setState(() {
        if (push) _path.add(location);
        _entries = entries;
        if (location.uri.isEmpty) {
          _registeredSchemes
            ..clear()
            ..addAll(
              entries
                  .where((entry) => entry.kind == 'scheme')
                  .map((entry) => entry.name),
            );
        }
      });
    } catch (_) {
      if (!mounted) return;
      setState(() {
        _error = '這個資源目錄目前不提供清單。';
        _failedLocation = location;
      });
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _open(MikuResourceEntry entry) async {
    if (_busy) return;
    if (_isInspectorContainer(entry)) {
      await _load(
        _ResourceLocation(
          uri: _inspectorLocationUri(entry),
          label: _resourceEntryLabel(entry),
        ),
        push: true,
      );
      return;
    }
    await _previewUri(entry.uri);
  }

  Future<void> _previewUri(String uri) async {
    if (_busy) return;
    setState(() {
      _loading = true;
      _error = null;
      _resourcePageError = null;
      _nextResourceLine = 1;
      _failedPreviewUri = null;
    });
    try {
      final preview = await widget.client.previewResource(
        widget.sessionId,
        uri,
      );
      if (!mounted) return;
      setState(() {
        _preview = preview;
        _nextResourceLine = 1;
      });
    } catch (_) {
      if (!mounted) return;
      setState(() {
        _error = '這個資源目前無法預覽。';
        _failedPreviewUri = uri;
      });
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _back() async {
    if (_preview != null || _failedPreviewUri != null) {
      setState(() {
        _preview = null;
        _resourcePageError = null;
        _nextResourceLine = 1;
        _failedPreviewUri = null;
        _error = null;
      });
      return;
    }
    if (_path.length <= 1) return;
    final target = _path[_path.length - 2];
    setState(() => _path.removeLast());
    await _load(target, push: false);
  }

  Future<void> _openExactUri() async {
    final uri = await showDialog<String>(
      context: context,
      builder:
          (context) => _ExactResourceUriDialog(
            registeredSchemes: Set.unmodifiable(_registeredSchemes),
          ),
    );
    if (uri != null && mounted) await _previewUri(uri);
  }

  Future<void> _loadMoreResource() async {
    final current = _preview;
    if (_busy || current == null || !_canLoadMoreResource(current)) return;
    final start = _nextResourceLine;
    final end = start + 199;
    final requestedSelector = '$start-$end';
    setState(() {
      _loadingResourcePage = true;
      _resourcePageError = null;
    });
    try {
      final page = await widget.client.resolveResource(
        widget.sessionId,
        current.uri,
        selector: requestedSelector,
      );
      if (!mounted) return;
      final selectedText =
          page.content.isNotEmpty ? page.content : page.preview;
      final priorText = current.selector == null ? '' : current.content;
      final returnedSelector =
          page.selector?.trim().isNotEmpty == true
              ? page.selector!.trim()
              : requestedSelector;
      setState(() {
        _preview = ResourcePreview(
          uri: page.uri,
          kind: page.kind,
          mime: page.mime,
          title: page.title ?? current.title,
          sizeBytes: page.sizeBytes,
          preview: '',
          content: _appendResourcePage(priorText, selectedText),
          selector: returnedSelector,
          hasMore: page.hasMore,
        );
        _nextResourceLine = _selectorEnd(returnedSelector, fallback: end) + 1;
      });
    } catch (_) {
      if (!mounted) return;
      setState(() {
        _resourcePageError = '下一段內容暫時載入失敗；目前內容已保留。';
      });
    } finally {
      if (mounted) setState(() => _loadingResourcePage = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    return PopScope(
      canPop: _preview == null && _path.length <= 1,
      onPopInvokedWithResult: (didPop, result) {
        if (!didPop) unawaited(_back());
      },
      child: FractionallySizedBox(
        key: const Key('resource-inspector'),
        heightFactor: 0.9,
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 760),
            child: Padding(
              padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      if (_path.length > 1 || _preview != null)
                        IconButton(
                          key: const Key('resource-back'),
                          tooltip: '返回上一層',
                          onPressed: _busy ? null : _back,
                          icon: const Icon(Icons.arrow_back_rounded),
                        ),
                      Expanded(
                        child: Text(
                          _preview?.title?.trim().isNotEmpty == true
                              ? _preview!.title!
                              : _preview != null
                              ? _preview!.uri
                              : _path.last.label,
                          key: const Key('resource-inspector-title'),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(context).textTheme.titleLarge
                              ?.copyWith(fontWeight: FontWeight.w600),
                        ),
                      ),
                      if (_preview == null && _path.last.uri.isNotEmpty)
                        IconButton(
                          key: const Key('preview-resource-location'),
                          tooltip: '預覽目前資源',
                          onPressed:
                              _busy ? null : () => _previewUri(_path.last.uri),
                          icon: const Icon(Icons.visibility_outlined),
                        ),
                      IconButton(
                        key: const Key('open-exact-resource-uri'),
                        tooltip: '開啟資源 URI',
                        onPressed: _busy ? null : _openExactUri,
                        icon: const Icon(Icons.link_rounded),
                      ),
                      IconButton(
                        tooltip: '關閉資源檢視器',
                        onPressed: () => Navigator.of(context).pop(),
                        icon: const Icon(Icons.close_rounded),
                      ),
                    ],
                  ),
                  Text(
                    _preview?.uri ??
                        (_path.last.uri.isEmpty ? '已授權的資源類型' : _path.last.uri),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      color: palette.muted,
                      fontFamily: 'monospace',
                    ),
                  ),
                  const SizedBox(height: 12),
                  if (_loading) const LinearProgressIndicator(minHeight: 2),
                  if (_error != null) ...[
                    const SizedBox(height: 8),
                    _DriveInlineError(
                      message: _error!,
                      onRetry: () {
                        final failedUri = _failedPreviewUri;
                        if (failedUri != null) {
                          unawaited(_previewUri(failedUri));
                          return;
                        }
                        unawaited(
                          _load(
                            _failedLocation ?? _path.last,
                            push: _failedLocation != null,
                          ),
                        );
                      },
                    ),
                  ],
                  const SizedBox(height: 8),
                  Expanded(
                    child:
                        _preview != null
                            ? _ResourcePreviewBody(
                              preview: _preview!,
                              loadingMore: _loadingResourcePage,
                              loadMoreError: _resourcePageError,
                              onLoadMore:
                                  _canLoadMoreResource(_preview!)
                                      ? _loadMoreResource
                                      : null,
                            )
                            : _ResourceEntryList(
                              entries: _entries,
                              loading: _loading,
                              onOpen: _open,
                            ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _ExactResourceUriDialog extends StatefulWidget {
  const _ExactResourceUriDialog({required this.registeredSchemes});

  final Set<String> registeredSchemes;

  @override
  State<_ExactResourceUriDialog> createState() =>
      _ExactResourceUriDialogState();
}

class _ExactResourceUriDialogState extends State<_ExactResourceUriDialog> {
  final TextEditingController _controller = TextEditingController();
  String? _validationError;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _submit() {
    final value = _controller.text.trim();
    final parsed = Uri.tryParse(value);
    if (parsed == null ||
        parsed.scheme.isEmpty ||
        !value.contains('://') ||
        (widget.registeredSchemes.isNotEmpty &&
            !widget.registeredSchemes.contains(parsed.scheme))) {
      setState(() => _validationError = '請輸入目前伺服器已註冊的完整資源 URI。');
      return;
    }
    Navigator.of(context).pop(value);
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('開啟資源 URI'),
      content: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 520),
        child: TextField(
          key: const Key('exact-resource-uri-input'),
          controller: _controller,
          autofocus: true,
          autocorrect: false,
          enableSuggestions: false,
          onSubmitted: (_) => _submit(),
          decoration: InputDecoration(
            hintText: 'history://…',
            helperText: '只會要求伺服器提供有界唯讀預覽。',
            errorText: _validationError,
            border: const OutlineInputBorder(),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(
          key: const Key('confirm-exact-resource-uri'),
          onPressed: _submit,
          child: const Text('開啟預覽'),
        ),
      ],
    );
  }
}

class _ResourceEntryList extends StatelessWidget {
  const _ResourceEntryList({
    required this.entries,
    required this.loading,
    required this.onOpen,
  });

  final List<MikuResourceEntry>? entries;
  final bool loading;
  final ValueChanged<MikuResourceEntry> onOpen;

  @override
  Widget build(BuildContext context) {
    final values = entries;
    if (values == null) {
      return loading
          ? const _DrawerLoadingState(label: '載入資源…')
          : const SizedBox.shrink();
    }
    if (values.isEmpty) {
      return const _DrawerEmptyState(text: '這個資源目錄目前是空的。');
    }
    return ListView.separated(
      key: const Key('resource-entry-list'),
      itemCount: values.length,
      separatorBuilder: (_, __) => const Divider(height: 1),
      itemBuilder: (context, index) {
        final entry = values[index];
        final navigates = _isInspectorContainer(entry);
        final unavailableRoot =
            entry.kind == 'scheme' &&
            (entry.name == 'history' || entry.name == 'linked');
        return ListTile(
          key: Key('resource-entry-${entry.uri}'),
          minTileHeight: 56,
          enabled: !loading && !unavailableRoot,
          leading: Icon(_inspectorResourceIcon(entry)),
          title: Text(
            _resourceEntryLabel(entry),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
          subtitle: Text(
            unavailableRoot
                ? entry.name == 'history'
                    ? '請使用上方連結按鈕開啟完整 history URI'
                    : '請從專案開啟已授權的 linked alias'
                : entry.kind,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
          trailing: Icon(
            navigates ? Icons.chevron_right_rounded : Icons.visibility_outlined,
          ),
          onTap: loading || unavailableRoot ? null : () => onOpen(entry),
        );
      },
    );
  }
}

class _ResourcePreviewBody extends StatelessWidget {
  const _ResourcePreviewBody({
    required this.preview,
    required this.loadingMore,
    required this.loadMoreError,
    required this.onLoadMore,
  });

  final ResourcePreview preview;
  final bool loadingMore;
  final String? loadMoreError;
  final VoidCallback? onLoadMore;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    final body =
        preview.preview.trim().isNotEmpty
            ? preview.preview
            : preview.content.trim().isNotEmpty
            ? preview.content
            : '這個資源沒有可顯示的文字預覽。';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          '${preview.mime.isEmpty ? preview.kind : preview.mime} · ${_formatBytes(preview.sizeBytes)}',
          style: Theme.of(
            context,
          ).textTheme.labelSmall?.copyWith(color: palette.muted),
        ),
        if (preview.hasMore) ...[
          const SizedBox(height: 8),
          Text(
            '內容超過安全預覽上限，僅顯示目前載入的部分。',
            key: const Key('resource-preview-truncated'),
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(color: palette.muted),
          ),
        ],
        if (onLoadMore != null) ...[
          const SizedBox(height: 10),
          OutlinedButton.icon(
            key: const Key('resource-load-more'),
            onPressed: loadingMore ? null : onLoadMore,
            style: OutlinedButton.styleFrom(
              minimumSize: const Size.fromHeight(44),
            ),
            icon:
                loadingMore
                    ? const SizedBox.square(
                      dimension: 18,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                    : const Icon(Icons.expand_more_rounded),
            label: Text(
              loadingMore
                  ? '載入中…'
                  : preview.selector == null
                  ? '載入第 1–200 行'
                  : '繼續載入下一段',
            ),
          ),
        ],
        if (loadMoreError != null) ...[
          const SizedBox(height: 6),
          Semantics(
            key: const Key('resource-load-more-error'),
            liveRegion: true,
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    loadMoreError!,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ),
                TextButton(
                  key: const Key('resource-load-more-retry'),
                  onPressed: loadingMore ? null : onLoadMore,
                  style: TextButton.styleFrom(minimumSize: const Size(64, 44)),
                  child: const Text('重試'),
                ),
              ],
            ),
          ),
        ],
        const SizedBox(height: 10),
        Expanded(
          child: DecoratedBox(
            decoration: BoxDecoration(
              border: Border.all(color: palette.outline),
              borderRadius: BorderRadius.circular(14),
            ),
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(16),
              child: SelectableText(
                body,
                key: const Key('resource-preview-content'),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

bool _canLoadMoreResource(ResourcePreview preview) {
  if (!preview.hasMore || !_isTextCompatibleResource(preview)) return false;
  return Uri.tryParse(preview.uri)?.scheme.toLowerCase() != 'skill';
}

bool _isTextCompatibleResource(ResourcePreview preview) {
  final mime = preview.mime.toLowerCase().split(';').first.trim();
  if (mime.startsWith('text/') ||
      mime == 'application/json' ||
      mime.endsWith('+json') ||
      mime == 'application/xml' ||
      mime.endsWith('+xml') ||
      mime == 'application/yaml' ||
      mime == 'application/toml' ||
      mime == 'application/javascript') {
    return true;
  }
  final kind = preview.kind.toLowerCase();
  return kind == 'text' || kind == 'drive_document' || kind == 'project_view';
}

String _appendResourcePage(String current, String next) {
  if (current.isEmpty) return next;
  if (next.isEmpty) return current;
  final separator = current.endsWith('\n') || next.startsWith('\n') ? '' : '\n';
  return '$current$separator$next';
}

int _selectorEnd(String selector, {required int fallback}) {
  final match = RegExp(r'^\d+-(\d+)$').firstMatch(selector);
  return int.tryParse(match?.group(1) ?? '') ?? fallback;
}

String _resourceEntryLabel(MikuResourceEntry entry) {
  final title = entry.title?.trim();
  return title?.isNotEmpty == true ? title! : entry.name;
}

IconData _inspectorResourceIcon(MikuResourceEntry entry) {
  if (entry.kind == 'scheme') {
    return switch (entry.name) {
      'memory' => Icons.psychology_outlined,
      'agent' => Icons.account_tree_outlined,
      'history' => Icons.history_rounded,
      'skill' => Icons.extension_outlined,
      'cron' => Icons.schedule_outlined,
      'mcp' => Icons.hub_outlined,
      'artifact' => Icons.inventory_2_outlined,
      _ => Icons.folder_open_outlined,
    };
  }
  if (entry.isDirectory) return Icons.folder_outlined;
  return Icons.description_outlined;
}

bool _isInspectorContainer(MikuResourceEntry entry) {
  if (entry.kind == 'scheme' || entry.isDirectory) return true;
  if (entry.kind.endsWith('_collection')) return true;
  return switch (entry.kind) {
    'managed_skill' ||
    'cron_job' ||
    'memory_user_model' ||
    'memory_dream_queue' => true,
    _ => false,
  };
}

String _inspectorLocationUri(MikuResourceEntry entry) {
  if (entry.kind == 'scheme' && entry.name == 'workspace') {
    return 'workspace://session/';
  }
  return entry.uri;
}
