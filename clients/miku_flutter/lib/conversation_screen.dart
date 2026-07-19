import 'dart:async';

import 'package:flutter/material.dart';

import 'rich_message.dart';
import 'session_models.dart';

const _showRichResponseShowcase = bool.fromEnvironment(
  'TM_RICH_RESPONSE_SHOWCASE',
);

class ConversationScreen extends StatefulWidget {
  const ConversationScreen({required this.client, this.now, super.key});

  final MikuSessionClient client;
  final DateTime Function()? now;

  @override
  State<ConversationScreen> createState() => _ConversationScreenState();
}

enum _PresenceState { loading, here, working, reconnecting, offline, ended }

enum _ServerConnectionState { connecting, connected, reconnecting, offline }

sealed class _ConversationItem {
  const _ConversationItem(this.key);

  final String key;
}

class _MessageItem extends _ConversationItem {
  _MessageItem({
    required String key,
    required this.role,
    required this.text,
    this.streaming = false,
  }) : super(key);

  final String role;
  String text;
  bool streaming;
}

class _ActivityItem extends _ConversationItem {
  _ActivityItem({required String key, required this.label, this.detail})
    : super(key);

  String label;
  String? detail;
  bool running = true;
}

class _ApprovalItem extends _ConversationItem {
  _ApprovalItem({required String key, required this.prompt}) : super(key);

  final ApprovalPrompt prompt;
  bool resolving = false;
  String? resolvedStatus;
  String? error;
}

class _NoticeItem extends _ConversationItem {
  const _NoticeItem({
    required String key,
    required this.text,
    this.isError = false,
  }) : super(key);

  final String text;
  final bool isError;
}

class _ConversationScreenState extends State<ConversationScreen> {
  final _composerController = TextEditingController();
  final _composerFocus = FocusNode();
  final _scrollController = ScrollController();
  final List<_ConversationItem> _items = [];

  StreamSubscription<MikuEvent>? _eventSubscription;
  MikuSession? _session;
  _PresenceState _presence = _PresenceState.loading;
  _ServerConnectionState _serverConnection = _ServerConnectionState.connecting;
  bool _sending = false;
  String? _connectionError;
  int _localSequence = 0;

  @override
  void initState() {
    super.initState();
    _composerController.addListener(_composerChanged);
    unawaited(_connect());
  }

  @override
  void dispose() {
    _eventSubscription?.cancel();
    _composerController
      ..removeListener(_composerChanged)
      ..dispose();
    _composerFocus.dispose();
    _scrollController.dispose();
    super.dispose();
  }

  void _composerChanged() {
    if (mounted) setState(() {});
  }

  String _nextKey(String prefix) => '$prefix-${_localSequence++}';

  Future<void> _connect() async {
    await _eventSubscription?.cancel();
    if (mounted) {
      setState(() {
        _presence = _PresenceState.loading;
        _serverConnection = _ServerConnectionState.connecting;
        _connectionError = null;
      });
    }
    try {
      final session = await widget.client.createOrReuseSession();
      final loaded = await widget.client.loadSession(session.id);
      if (!mounted) return;
      final restored = <_ConversationItem>[
        for (final message in loaded.messages)
          _MessageItem(
            key: 'history-${message.seq}',
            role: message.role,
            text: message.content,
          ),
        if (_showRichResponseShowcase)
          _MessageItem(
            key: 'rich-response-showcase',
            role: 'assistant',
            text: mikuRichResponseShowcase,
          ),
      ];
      setState(() {
        _session = loaded.session;
        _items
          ..clear()
          ..addAll(restored);
        _presence =
            loaded.session.status == 'ended'
                ? _PresenceState.ended
                : _PresenceState.here;
        _serverConnection = _ServerConnectionState.connected;
      });
      for (final event in loaded.pendingEvents) {
        _handleEvent(event, remember: false);
      }
      if (_presence != _PresenceState.ended) {
        _listenForEvents(loaded.session.id, loaded.session.lastEventId);
      }
      _scheduleScroll(force: true);
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _presence = _PresenceState.offline;
        _serverConnection = _ServerConnectionState.offline;
        _connectionError = _friendlyError(error);
      });
    }
  }

  void _listenForEvents(String sessionId, String? lastEventId) {
    _eventSubscription = widget.client
        .events(sessionId, lastEventId: lastEventId)
        .listen(
          _handleEvent,
          onError: (Object error, StackTrace stackTrace) {
            if (!mounted) return;
            setState(() {
              _presence = _PresenceState.offline;
              _serverConnection = _ServerConnectionState.offline;
              _connectionError = _friendlyError(error);
            });
          },
          onDone: () {
            if (!mounted || _presence == _PresenceState.ended) return;
            setState(() {
              _presence = _PresenceState.offline;
              _serverConnection = _ServerConnectionState.offline;
              _connectionError = '連線中斷了。你的對話仍然保留著。';
            });
          },
        );
  }

  void _handleEvent(MikuEvent event, {bool remember = true}) {
    if (!mounted) return;
    setState(() {
      switch (event.type) {
        case 'text':
          _appendTextDelta(_string(event.data['delta']));
          _presence = _PresenceState.working;
        case 'final':
          _finishAssistantMessage(_string(event.data['text']));
          _finishActivities();
          _presence = _PresenceState.here;
        case 'tool_call':
          _addActivity(event, '正在處理', detail: _string(event.data['name']));
          _presence = _PresenceState.working;
        case 'cell_start':
          _addActivity(event, '正在執行', detail: '安全工作環境');
          _presence = _PresenceState.working;
        case 'effect_start':
          _addActivity(event, '正在執行', detail: '受控能力');
          _presence = _PresenceState.working;
        case 'effect_suspended':
          _pauseLatestActivity();
          _presence = _PresenceState.here;
        case 'effect_resumed':
          _resumeLatestActivity();
          _presence = _PresenceState.working;
        case 'actor_spawned':
          _addActivity(event, '正在分工處理', detail: _string(event.data['task']));
          _presence = _PresenceState.working;
        case 'reasoning':
          _addActivity(event, '正在想一想');
          _presence = _PresenceState.working;
        case 'progress':
          _addActivity(
            event,
            _string(event.data['label']).isEmpty
                ? '正在處理'
                : _string(event.data['label']),
          );
          _presence = _PresenceState.working;
        case 'actor_completed':
        case 'cell_result':
        case 'effect_end':
        case 'effect_result':
          _completeLatestActivity(event);
        case 'mcp_invocation':
          if (_string(event.data['status']) == 'requested') {
            _addActivity(event, '正在查詢外部資源');
            _presence = _PresenceState.working;
          } else {
            _completeLatestActivity(event);
          }
        case 'approval':
          final prompt = _approvalFromEvent(event);
          if (prompt != null &&
              !_items.whereType<_ApprovalItem>().any(
                (item) => item.prompt.approvalId == prompt.approvalId,
              )) {
            _items.add(
              _ApprovalItem(
                key: 'approval-${prompt.approvalId}',
                prompt: prompt,
              ),
            );
          }
          _presence = _PresenceState.here;
        case 'approval_resolved':
          final approvalId = _string(event.data['approvalId']);
          final status = _string(event.data['status']);
          for (final item in _items.whereType<_ApprovalItem>()) {
            if (item.prompt.approvalId == approvalId) {
              item
                ..resolving = false
                ..resolvedStatus = status;
            }
          }
        case 'runtime_reset':
          _items.add(
            _NoticeItem(key: _nextKey('runtime-reset'), text: '執行環境已重新連線。'),
          );
        case 'error':
          _items.add(
            _NoticeItem(
              key: event.id ?? _nextKey('error'),
              text:
                  _string(event.data['message']).isEmpty
                      ? '這一步沒有完成，可以再試一次。'
                      : _string(event.data['message']),
              isError: true,
            ),
          );
          _presence = _PresenceState.here;
        case 'session_end':
          _finishActivities();
          _presence = _PresenceState.ended;
        case 'connection':
          _updateConnectionState(_string(event.data['status']));
        case 'mode':
        case 'write_proposal':
          break;
        default:
          break;
      }
    });
    final session = _session;
    if (remember &&
        session != null &&
        event.id != null &&
        shouldRememberEventId(event.type, event.data)) {
      widget.client.rememberLastEventId(session.id, event.id!);
    }
    if (event.type == 'session_end') {
      unawaited(_eventSubscription?.cancel());
    }
    _scheduleScroll();
  }

  void _appendTextDelta(String delta) {
    if (delta.isEmpty) return;
    final last = _items.isEmpty ? null : _items.last;
    if (last is _MessageItem && last.role == 'assistant' && last.streaming) {
      last.text += delta;
      return;
    }
    _items.add(
      _MessageItem(
        key: _nextKey('assistant'),
        role: 'assistant',
        text: delta,
        streaming: true,
      ),
    );
  }

  void _finishAssistantMessage(String text) {
    for (final item in _items.reversed) {
      if (item is _MessageItem && item.role == 'assistant' && item.streaming) {
        if (text.isNotEmpty) item.text = text;
        item.streaming = false;
        return;
      }
    }
    if (text.isEmpty) return;
    final last = _items.isEmpty ? null : _items.last;
    if (last is _MessageItem && last.role == 'assistant' && last.text == text) {
      return;
    }
    _items.add(
      _MessageItem(key: _nextKey('assistant'), role: 'assistant', text: text),
    );
  }

  void _addActivity(MikuEvent event, String label, {String? detail}) {
    _items.add(
      _ActivityItem(
        key: event.id ?? _nextKey('activity'),
        label: label,
        detail: detail?.isEmpty == true ? null : detail,
      ),
    );
  }

  void _completeLatestActivity(MikuEvent event) {
    for (final item in _items.reversed.whereType<_ActivityItem>()) {
      if (!item.running) continue;
      item.running = false;
      final result =
          _string(event.data['resultPreview']).isNotEmpty
              ? _string(event.data['resultPreview'])
              : _string(event.data['summary']);
      if (result.isNotEmpty) item.detail = result;
      return;
    }
  }

  void _pauseLatestActivity() {
    for (final item in _items.reversed.whereType<_ActivityItem>()) {
      if (!item.running) continue;
      item
        ..running = false
        ..label = '等待確認';
      return;
    }
  }

  void _resumeLatestActivity() {
    for (final item in _items.reversed.whereType<_ActivityItem>()) {
      if (item.running || item.label != '等待確認') continue;
      item
        ..running = true
        ..label = '繼續執行';
      return;
    }
  }

  void _updateConnectionState(String status) {
    switch (status) {
      case 'connected':
        _serverConnection = _ServerConnectionState.connected;
        _connectionError = null;
        if (_presence == _PresenceState.loading ||
            _presence == _PresenceState.reconnecting ||
            _presence == _PresenceState.offline) {
          _presence = _PresenceState.here;
        }
      case 'reconnecting':
        _serverConnection = _ServerConnectionState.reconnecting;
        _presence = _PresenceState.reconnecting;
        _connectionError = '連線不穩，正在重新連線。';
      case 'offline':
        _serverConnection = _ServerConnectionState.offline;
        _presence = _PresenceState.offline;
        _connectionError = '現在連不上 Miku。你的對話仍然保留著。';
      default:
        break;
    }
  }

  void _finishActivities() {
    for (final item in _items.whereType<_ActivityItem>()) {
      item.running = false;
    }
  }

  Future<void> _send() async {
    final session = _session;
    final content = _composerController.text.trim();
    if (session == null ||
        content.isEmpty ||
        _sending ||
        _presence == _PresenceState.loading ||
        _presence == _PresenceState.offline ||
        _presence == _PresenceState.ended) {
      return;
    }
    final item = _MessageItem(
      key: _nextKey('user'),
      role: 'user',
      text: content,
    );
    setState(() {
      _items.add(item);
      _composerController.clear();
      _sending = true;
      _connectionError = null;
    });
    _scheduleScroll(force: true);
    try {
      await widget.client.sendMessage(
        session.id,
        content,
        clientMessageId: newClientMessageId(),
      );
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _items.remove(item);
        _composerController.text = content;
        _composerController.selection = TextSelection.collapsed(
          offset: content.length,
        );
        _connectionError = '沒有送出去。內容已經放回輸入框。';
      });
    } finally {
      if (mounted) setState(() => _sending = false);
    }
  }

  Future<void> _resolveApproval(
    _ApprovalItem item,
    ApprovalOption option,
  ) async {
    final session = _session;
    if (session == null || item.resolving || item.resolvedStatus != null) {
      return;
    }
    final approve =
        option.kind.contains('allow') || option.kind.contains('approve');
    setState(() {
      item
        ..resolving = true
        ..error = null;
    });
    try {
      await widget.client.resolveApproval(
        session.id,
        item.prompt.approvalId,
        approve ? 'approve' : 'deny',
        optionId: option.optionId,
      );
    } catch (error) {
      if (!mounted) return;
      setState(() {
        item
          ..resolving = false
          ..error = '沒有完成這個決定，請再試一次。';
      });
    }
  }

  ApprovalPrompt? _approvalFromEvent(MikuEvent event) {
    final approvalId = _string(event.data['approvalId']);
    if (approvalId.isEmpty) return null;
    final rawScope = event.data['scope'];
    final scope =
        rawScope is Map
            ? rawScope.map((key, value) => MapEntry(key.toString(), value))
            : <String, Object?>{};
    final rawOptions = event.data['options'];
    final options = <ApprovalOption>[];
    if (rawOptions is List) {
      for (final raw in rawOptions.whereType<Map>()) {
        final option = raw.map((key, value) => MapEntry(key.toString(), value));
        final id = _string(option['optionId']);
        if (id.isEmpty) continue;
        options.add(
          ApprovalOption(
            optionId: id,
            name:
                _string(option['name']).isEmpty
                    ? _fallbackOptionName(_string(option['kind']))
                    : _string(option['name']),
            kind: _string(option['kind']),
          ),
        );
      }
    }
    return ApprovalPrompt(
      approvalId: approvalId,
      backend: _string(event.data['backend']),
      action:
          _string(event.data['action']).isEmpty
              ? '需要你的確認'
              : _string(event.data['action']),
      scope: scope,
      options:
          options.isEmpty
              ? const [
                ApprovalOption(
                  optionId: 'allow',
                  name: '允許一次',
                  kind: 'allow_once',
                ),
                ApprovalOption(
                  optionId: 'reject',
                  name: '拒絕',
                  kind: 'reject_once',
                ),
              ]
              : options,
      timeoutMs: event.data['timeoutMs'] as int?,
    );
  }

  void _scheduleScroll({bool force = false}) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_scrollController.hasClients) return;
      final position = _scrollController.position;
      final nearBottom = position.maxScrollExtent - position.pixels < 160;
      if (!force && !nearBottom) return;
      if (MediaQuery.maybeOf(context)?.disableAnimations ?? false) {
        position.jumpTo(position.maxScrollExtent);
        return;
      }
      _scrollController.animateTo(
        position.maxScrollExtent,
        duration: const Duration(milliseconds: 180),
        curve: Curves.easeOutCubic,
      );
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Scaffold(
      body: SafeArea(
        child: LayoutBuilder(
          builder: (context, constraints) {
            final horizontalPadding = constraints.maxWidth < 600 ? 16.0 : 28.0;
            return Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 820),
                child: Padding(
                  padding: EdgeInsets.symmetric(horizontal: horizontalPadding),
                  child: Column(
                    children: [
                      _PresenceBar(connection: _serverConnection),
                      Divider(height: 1, color: palette.outline),
                      Expanded(child: _buildConversation(palette)),
                      if (_connectionError != null)
                        _ConnectionNotice(
                          text: _connectionError!,
                          canRetry: _presence == _PresenceState.offline,
                          onRetry: _connect,
                        ),
                      _Composer(
                        controller: _composerController,
                        focusNode: _composerFocus,
                        enabled: _canCompose,
                        disabledHint: _disabledComposerHint,
                        sending: _sending,
                        onSend: _send,
                      ),
                    ],
                  ),
                ),
              ),
            );
          },
        ),
      ),
    );
  }

  Widget _buildConversation(_Palette palette) {
    if (_presence == _PresenceState.loading && _items.isEmpty) {
      return const Center(child: _QuietLoading());
    }
    if (_items.isEmpty) {
      return Semantics(
        liveRegion: true,
        label: 'Miku is here',
        child: Center(
          child: Padding(
            padding: const EdgeInsets.only(bottom: 48),
            child: Text(
              '${_greeting()}。我在這裡。',
              key: const Key('empty-presence-copy'),
              textAlign: TextAlign.center,
              style: Theme.of(context).textTheme.titleMedium?.copyWith(
                color: palette.muted,
                fontWeight: FontWeight.w400,
              ),
            ),
          ),
        ),
      );
    }
    return ListView.builder(
      key: const Key('conversation-list'),
      controller: _scrollController,
      padding: const EdgeInsets.symmetric(vertical: 24),
      keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
      itemCount: _items.length,
      itemBuilder: (context, index) {
        final item = _items[index];
        return Padding(
          padding: const EdgeInsets.only(bottom: 18),
          child: switch (item) {
            _MessageItem message => _MessageRow(message: message),
            _ActivityItem activity => _ActivityRow(activity: activity),
            _ApprovalItem approval => _ApprovalCard(
              item: approval,
              onSelect: (option) => _resolveApproval(approval, option),
            ),
            _NoticeItem notice => _InlineNotice(notice: notice),
          },
        );
      },
    );
  }

  bool get _canCompose =>
      _session != null &&
      _presence != _PresenceState.loading &&
      _presence != _PresenceState.reconnecting &&
      _presence != _PresenceState.offline &&
      _presence != _PresenceState.ended;

  String get _disabledComposerHint => switch (_presence) {
    _PresenceState.loading => '正在找 Miku…',
    _PresenceState.reconnecting => '重新連線後再說…',
    _PresenceState.offline => '重新連線後再說…',
    _PresenceState.ended => '這段對話已結束',
    _PresenceState.here || _PresenceState.working => '告訴 Miku…',
  };

  String _greeting() {
    final hour = (widget.now?.call() ?? DateTime.now()).hour;
    if (hour < 5) return '還沒睡呀';
    if (hour < 11) return '早安';
    if (hour < 18) return '午安';
    return '晚上好';
  }
}

class _PresenceBar extends StatelessWidget {
  const _PresenceBar({required this.connection});

  final _ServerConnectionState connection;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final status = switch (connection) {
      _ServerConnectionState.connecting => '正在連上伺服器',
      _ServerConnectionState.connected => '伺服器已連線',
      _ServerConnectionState.reconnecting => '正在重新連線',
      _ServerConnectionState.offline => '伺服器未連線',
    };
    return Semantics(
      container: true,
      liveRegion: true,
      label: status,
      child: SizedBox(
        height: 68,
        child: Row(
          children: [
            _PresenceMark(
              active: connection == _ServerConnectionState.connected,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'Miku',
                    style: Theme.of(context).textTheme.titleMedium?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 1),
                  Text(
                    status,
                    key: const Key('presence-status'),
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _PresenceMark extends StatelessWidget {
  const _PresenceMark({required this.active});

  final bool active;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      key: const Key('miku-presence-mark'),
      width: 34,
      height: 34,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: palette.miku.withValues(alpha: active ? 0.16 : 0.07),
        border: Border.all(
          color: palette.miku.withValues(alpha: active ? 0.7 : 0.25),
        ),
      ),
      alignment: Alignment.center,
      child: Container(
        width: 9,
        height: 9,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: active ? palette.miku : palette.muted,
        ),
      ),
    );
  }
}

class _MessageRow extends StatelessWidget {
  const _MessageRow({required this.message});

  final _MessageItem message;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final user = message.role == 'user';
    final body =
        user
            ? SelectableText(
              message.text,
              key: Key('message-${message.key}'),
              style: Theme.of(context).textTheme.bodyLarge,
            )
            : MikuRichMessage(
              key: Key('message-${message.key}'),
              data: message.text,
            );
    return Semantics(
      liveRegion: message.streaming,
      label: user ? '你說：${message.text}' : null,
      child: Align(
        alignment: user ? Alignment.centerRight : Alignment.centerLeft,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: user ? 560 : 690),
          child:
              user
                  ? DecoratedBox(
                    decoration: BoxDecoration(
                      color: palette.userBubble,
                      borderRadius: const BorderRadius.only(
                        topLeft: Radius.circular(20),
                        topRight: Radius.circular(7),
                        bottomLeft: Radius.circular(20),
                        bottomRight: Radius.circular(20),
                      ),
                      border: Border.all(color: palette.outline),
                    ),
                    child: Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 16,
                        vertical: 11,
                      ),
                      child: body,
                    ),
                  )
                  : Padding(
                    padding: const EdgeInsets.only(left: 3, right: 16),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.end,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Flexible(child: body),
                        if (message.streaming) ...[
                          const SizedBox(width: 7),
                          _StreamingDot(color: palette.miku),
                        ],
                      ],
                    ),
                  ),
        ),
      ),
    );
  }
}

class _StreamingDot extends StatefulWidget {
  const _StreamingDot({required this.color});

  final Color color;

  @override
  State<_StreamingDot> createState() => _StreamingDotState();
}

class _StreamingDotState extends State<_StreamingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 900),
      lowerBound: 0.35,
      upperBound: 1,
    )..repeat(reverse: true);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (MediaQuery.disableAnimationsOf(context)) {
      _controller.stop();
    } else if (!_controller.isAnimating) {
      _controller.repeat(reverse: true);
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return FadeTransition(
      opacity: _controller,
      child: Container(
        width: 6,
        height: 6,
        decoration: BoxDecoration(color: widget.color, shape: BoxShape.circle),
      ),
    );
  }
}

class _ActivityRow extends StatelessWidget {
  const _ActivityRow({required this.activity});

  final _ActivityItem activity;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      liveRegion: activity.running,
      label:
          '${activity.label}${activity.detail == null ? '' : '，${activity.detail}'}',
      child: Padding(
        padding: const EdgeInsets.only(left: 3),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: SizedBox(
                width: 12,
                height: 12,
                child:
                    activity.running
                        ? CircularProgressIndicator(
                          strokeWidth: 1.6,
                          color: palette.miku,
                        )
                        : Icon(
                          Icons.check_rounded,
                          size: 13,
                          color: palette.miku,
                        ),
              ),
            ),
            const SizedBox(width: 9),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    activity.label,
                    style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      color: palette.muted,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  if (activity.detail != null)
                    Text(
                      activity.detail!,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: palette.muted.withValues(alpha: 0.78),
                      ),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ApprovalCard extends StatelessWidget {
  const _ApprovalCard({required this.item, required this.onSelect});

  final _ApprovalItem item;
  final ValueChanged<ApprovalOption> onSelect;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final resolved = item.resolvedStatus;
    return Semantics(
      liveRegion: true,
      container: true,
      label: '需要確認：${item.prompt.action}',
      child: Container(
        key: Key('approval-${item.prompt.approvalId}'),
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          color: palette.approvalSurface,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(color: palette.approvalOutline),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.shield_outlined, size: 18, color: palette.warm),
                const SizedBox(width: 8),
                Text('需要你的確認', style: Theme.of(context).textTheme.labelLarge),
              ],
            ),
            const SizedBox(height: 10),
            SelectableText(
              item.prompt.action,
              style: Theme.of(context).textTheme.bodyMedium,
            ),
            if (_scopeLabel(item.prompt.scope) case final scope?) ...[
              const SizedBox(height: 5),
              Text(
                scope,
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(color: palette.muted),
              ),
            ],
            if (item.error != null) ...[
              const SizedBox(height: 8),
              Text(
                item.error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            ],
            const SizedBox(height: 14),
            if (resolved != null)
              Text(
                resolved == 'approved' ? '已允許' : '已拒絕',
                key: const Key('approval-resolution'),
                style: Theme.of(context).textTheme.labelLarge?.copyWith(
                  color: resolved == 'approved' ? palette.miku : palette.muted,
                ),
              )
            else
              Wrap(
                spacing: 10,
                runSpacing: 8,
                children: [
                  for (final option in item.prompt.options)
                    _ApprovalButton(
                      option: option,
                      enabled: !item.resolving,
                      onPressed: () => onSelect(option),
                    ),
                  if (item.resolving)
                    const Padding(
                      padding: EdgeInsets.all(10),
                      child: SizedBox.square(
                        dimension: 17,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      ),
                    ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

class _ApprovalButton extends StatelessWidget {
  const _ApprovalButton({
    required this.option,
    required this.enabled,
    required this.onPressed,
  });

  final ApprovalOption option;
  final bool enabled;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final approve =
        option.kind.contains('allow') || option.kind.contains('approve');
    if (approve) {
      return FilledButton(
        key: Key('approval-option-${option.optionId}'),
        onPressed: enabled ? onPressed : null,
        child: Text(option.name),
      );
    }
    return OutlinedButton(
      key: Key('approval-option-${option.optionId}'),
      onPressed: enabled ? onPressed : null,
      child: Text(option.name),
    );
  }
}

class _InlineNotice extends StatelessWidget {
  const _InlineNotice({required this.notice});

  final _NoticeItem notice;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      liveRegion: notice.isError,
      child: Text(
        notice.text,
        style: Theme.of(context).textTheme.bodySmall?.copyWith(
          color:
              notice.isError
                  ? Theme.of(context).colorScheme.error
                  : palette.muted,
        ),
      ),
    );
  }
}

class _ConnectionNotice extends StatelessWidget {
  const _ConnectionNotice({
    required this.text,
    required this.canRetry,
    required this.onRetry,
  });

  final String text;
  final bool canRetry;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      liveRegion: true,
      child: Padding(
        padding: const EdgeInsets.only(top: 8),
        child: Row(
          children: [
            Expanded(
              child: Text(
                text,
                key: const Key('connection-notice'),
                style: Theme.of(context).textTheme.bodySmall?.copyWith(
                  color: Theme.of(context).colorScheme.error,
                ),
              ),
            ),
            if (canRetry)
              TextButton(
                key: const Key('retry-connection'),
                onPressed: onRetry,
                child: const Text('重新連線'),
              ),
          ],
        ),
      ),
    );
  }
}

class _Composer extends StatelessWidget {
  const _Composer({
    required this.controller,
    required this.focusNode,
    required this.enabled,
    required this.disabledHint,
    required this.sending,
    required this.onSend,
  });

  final TextEditingController controller;
  final FocusNode focusNode;
  final bool enabled;
  final String disabledHint;
  final bool sending;
  final VoidCallback onSend;

  @override
  Widget build(BuildContext context) {
    final canSend = enabled && !sending && controller.text.trim().isNotEmpty;
    final colors = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(0, 10, 0, 12),
      child: Semantics(
        textField: true,
        label: '告訴 Miku',
        child: TextField(
          key: const Key('conversation-composer'),
          controller: controller,
          focusNode: focusNode,
          enabled: enabled,
          minLines: 1,
          maxLines: 6,
          textCapitalization: TextCapitalization.sentences,
          keyboardType: TextInputType.multiline,
          textInputAction: TextInputAction.newline,
          decoration: InputDecoration(
            hintText: enabled ? '告訴 Miku…' : disabledHint,
            suffixIcon: Padding(
              padding: const EdgeInsets.all(5),
              child: IconButton.filled(
                key: const Key('send-message'),
                tooltip: '送出',
                onPressed: canSend ? onSend : null,
                style: IconButton.styleFrom(
                  backgroundColor: colors.primary,
                  foregroundColor: colors.onPrimary,
                  disabledBackgroundColor: colors.onSurface.withValues(
                    alpha: 0.12,
                  ),
                  disabledForegroundColor: colors.onSurface.withValues(
                    alpha: 0.38,
                  ),
                ),
                icon:
                    sending
                        ? const SizedBox.square(
                          dimension: 18,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                        : const Icon(Icons.arrow_upward_rounded),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _QuietLoading extends StatelessWidget {
  const _QuietLoading();

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      liveRegion: true,
      label: '正在載入對話',
      child: SizedBox.square(
        dimension: 24,
        child: CircularProgressIndicator(strokeWidth: 2, color: palette.miku),
      ),
    );
  }
}

class _Palette {
  const _Palette({
    required this.miku,
    required this.muted,
    required this.outline,
    required this.userBubble,
    required this.warm,
    required this.approvalSurface,
    required this.approvalOutline,
  });

  final Color miku;
  final Color muted;
  final Color outline;
  final Color userBubble;
  final Color warm;
  final Color approvalSurface;
  final Color approvalOutline;

  static _Palette of(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    if (dark) {
      return const _Palette(
        miku: Color(0xff5fd0c5),
        muted: Color(0xff9aa8ae),
        outline: Color(0xff28353b),
        userBubble: Color(0xff1a292f),
        warm: Color(0xffffc786),
        approvalSurface: Color(0xff211c18),
        approvalOutline: Color(0xff5d4934),
      );
    }
    return const _Palette(
      miku: Color(0xff167f78),
      muted: Color(0xff657378),
      outline: Color(0xffd9dfdd),
      userBubble: Color(0xffe4efeb),
      warm: Color(0xff9a5c18),
      approvalSurface: Color(0xfffff7ed),
      approvalOutline: Color(0xffe4c49d),
    );
  }
}

String _string(Object? value) => value?.toString() ?? '';

String _friendlyError(Object error) {
  final message = error.toString().replaceFirst(RegExp(r'^\w+Exception: '), '');
  if (message.trim().isEmpty) return '現在連不上 Miku，請稍後再試。';
  return '現在連不上 Miku。$message';
}

String _fallbackOptionName(String kind) {
  if (kind.contains('allow') || kind.contains('approve')) return '允許一次';
  return '拒絕';
}

String? _scopeLabel(Map<String, Object?> scope) {
  final capability = _string(scope['capability']);
  final actor = _string(scope['actorId']);
  if (capability.isNotEmpty && actor.isNotEmpty) return '$actor · $capability';
  if (capability.isNotEmpty) return capability;
  final proposal = scope['proposal'];
  if (proposal is Map) {
    final text = _string(proposal['text']);
    if (text.isNotEmpty) return text;
  }
  return null;
}
