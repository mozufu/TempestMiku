part of '../session_models.dart';

class TurnReceipt {
  const TurnReceipt({
    required this.turnId,
    required this.clientMessageId,
    required this.status,
  });

  final String turnId;
  final String clientMessageId;
  final String status;

  bool get isTerminal => _terminalTurnStatuses.contains(status);

  static TurnReceipt fromJson(Map<String, Object?> json) {
    return TurnReceipt(
      turnId: _stringValue(json['turnId'] ?? json['turn_id']),
      clientMessageId: _stringValue(
        json['clientMessageId'] ?? json['client_message_id'],
      ),
      status: _stringValue(json['status']),
    );
  }
}

class SessionTurn {
  const SessionTurn({
    required this.id,
    required this.sessionId,
    required this.clientMessageId,
    required this.content,
    required this.contentHash,
    required this.status,
    required this.createdAt,
    required this.updatedAt,
    this.startedAt,
    this.completedAt,
    this.workerId,
    this.error,
  });

  final String id;
  final String sessionId;
  final String clientMessageId;
  final String content;
  final String contentHash;
  final String status;
  final String createdAt;
  final String updatedAt;
  final String? startedAt;
  final String? completedAt;
  final String? workerId;
  final String? error;

  bool get isTerminal => _terminalTurnStatuses.contains(status);

  static SessionTurn fromJson(Map<String, Object?> json) {
    return SessionTurn(
      id: _stringValue(json['id']),
      sessionId: _stringValue(json['sessionId'] ?? json['session_id']),
      clientMessageId: _stringValue(
        json['clientMessageId'] ?? json['client_message_id'],
      ),
      content: _stringValue(json['content']),
      contentHash: _stringValue(json['contentHash'] ?? json['content_hash']),
      status: _stringValue(json['status']),
      createdAt: _stringValue(json['createdAt'] ?? json['created_at']),
      updatedAt: _stringValue(json['updatedAt'] ?? json['updated_at']),
      startedAt: _nullableString(json['startedAt'] ?? json['started_at']),
      completedAt: _nullableString(json['completedAt'] ?? json['completed_at']),
      workerId: _nullableString(json['workerId'] ?? json['worker_id']),
      error: _nullableString(json['error']),
    );
  }
}

const _terminalTurnStatuses = {'completed', 'failed', 'cancelled', 'timed_out'};
