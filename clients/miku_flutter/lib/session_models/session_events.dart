part of '../session_models.dart';

class MikuSession {
  const MikuSession({
    required this.id,
    required this.mode,
    required this.label,
    this.status = 'active',
    this.defaultScope = 'global',
    this.activeSkills = const [],
    this.lastEventId,
    this.locked = false,
  });

  final String id;
  final String status;
  final String mode;
  final String label;
  final String defaultScope;
  final List<String> activeSkills;
  final String? lastEventId;
  final bool locked;
}

class MikuEvent {
  const MikuEvent({
    required this.type,
    required this.data,
    this.id,
    this.turnId,
    this.createdAt,
  });

  final String type;
  final Map<String, Object?> data;
  final String? id;
  final String? turnId;
  final String? createdAt;
}

class SessionSummary {
  const SessionSummary({
    required this.id,
    required this.title,
    required this.preview,
    required this.mode,
    required this.label,
    required this.updatedAt,
    required this.status,
    required this.messageCount,
    this.lastEventId,
  });

  final String id;
  final String title;
  final String preview;
  final String mode;
  final String label;
  final String updatedAt;
  final String status;
  final int messageCount;
  final String? lastEventId;
}

class SessionMessage {
  const SessionMessage({
    required this.seq,
    required this.role,
    required this.content,
    required this.createdAt,
  });

  final int seq;
  final String role;
  final String content;
  final String createdAt;
}

class LoadedSession {
  const LoadedSession({
    required this.session,
    required this.messages,
    required this.pendingEvents,
  });

  final MikuSession session;
  final List<SessionMessage> messages;
  final List<MikuEvent> pendingEvents;
}

bool shouldRememberEventId(String type, Map<String, Object?> data) {
  if (type == 'approval') return false;
  if (type == 'write_proposal') {
    return _stringValue(data['status']) != 'pending';
  }
  return true;
}
