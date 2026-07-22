part of 'conversation_screen.dart';

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
  _ActivityItem({
    required String key,
    required this.label,
    this.detail,
    this.correlationKey,
    this.phase = _ActivityPhase.running,
    List<_ActivityResourceLink> links = const [],
  }) : links = List.of(links),
       super(key);

  String label;
  String? detail;
  final String? correlationKey;
  _ActivityPhase phase;
  final List<_ActivityResourceLink> links;

  bool get running => phase == _ActivityPhase.running;

  set running(bool value) {
    if (value) {
      phase = _ActivityPhase.running;
    } else if (phase == _ActivityPhase.running) {
      phase = _ActivityPhase.completed;
    }
  }
}

class _TurnItem extends _ConversationItem {
  _TurnItem({
    required String key,
    required this.clientMessageId,
    required this.status,
    this.turnId,
    this.error,
  }) : super(key);

  final String clientMessageId;
  String status;
  String? turnId;
  String? error;

  bool get isTerminal =>
      const {'completed', 'failed', 'cancelled', 'timed_out'}.contains(status);
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

sealed class _RenderNode {
  const _RenderNode();
}

class _ItemNode extends _RenderNode {
  const _ItemNode(this.item);

  final _ConversationItem item;
}

class _ActivityGroupNode extends _RenderNode {
  _ActivityGroupNode(this.activities);

  final List<_ActivityItem> activities;

  String get key => activities.first.correlationKey ?? activities.first.key;

  bool get hasActive => activities.any(
    (a) =>
        a.phase == _ActivityPhase.running || a.phase == _ActivityPhase.paused,
  );
}
