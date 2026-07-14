part of 'main.dart';

// ─── Conversation round model ──────────────────────────────────────────────────

class _ConversationRound {
  _ConversationRound({
    required this.index,
    required this.userText,
    this.isStreaming = true,
  });

  final int index;
  final String userText;
  final List<_ActivityItem> activities = [];
  String assistantStreamedText = '';
  String assistantFinalText = '';

  /// Private chain-of-thought the provider returned alongside the answer. Streamed in
  /// `reasoning` events; persists so the user can expand it after the turn completes.
  String reasoningText = '';
  bool isStreaming;
  bool activityExpanded = false;
  bool reasoningExpanded = false;

  String get assistantText =>
      assistantFinalText.isNotEmpty
          ? assistantFinalText
          : assistantStreamedText;

  bool get isComplete => assistantFinalText.isNotEmpty && !isStreaming;
  bool get hasReasoning => reasoningText.trim().isNotEmpty;
}

enum _ActivityState { running, done, failed, info }

class _ActivityItem {
  const _ActivityItem({
    required this.icon,
    required this.title,
    required this.detail,
    required this.state,
    this.monospace = false,
    this.kind = 'event',
    this.actorId,
    this.role,
    this.resourceUris = const [],
  });

  final IconData icon;
  final String title;
  final String detail;
  final _ActivityState state;
  final bool monospace;
  final String kind;
  final String? actorId;
  final String? role;
  final List<String> resourceUris;
}

class _AgentStatus {
  const _AgentStatus({
    required this.id,
    required this.role,
    required this.state,
    required this.detail,
  });

  final String id;
  final String role;
  final _ActivityState state;
  final String detail;

  bool get isRunning => state == _ActivityState.running;
}

// ─── Home page ─────────────────────────────────────────────────────────────────
