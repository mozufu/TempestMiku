import 'dart:convert';

import 'session_models.dart';

const sessionEventSseName = 'session_event';

/// Incrementally decodes the one versioned SSE shape accepted by TempestMiku clients.
///
/// Transport adapters feed decoded UTF-8 text chunks into this class. Keeping framing and
/// envelope validation here makes native HTTP and browser fetch behave identically.
class SessionEventSseDecoder {
  String _buffer = '';
  String _eventName = 'message';
  String? _id;
  final List<String> _dataLines = [];

  List<MikuEvent> add(String chunk) {
    _buffer += chunk;
    final events = <MikuEvent>[];
    while (true) {
      final newline = _buffer.indexOf('\n');
      if (newline < 0) break;
      var line = _buffer.substring(0, newline);
      _buffer = _buffer.substring(newline + 1);
      if (line.endsWith('\r')) line = line.substring(0, line.length - 1);
      final event = _consumeLine(line);
      if (event != null) events.add(event);
    }
    return events;
  }

  List<MikuEvent> close() {
    final events = <MikuEvent>[];
    if (_buffer.isNotEmpty) {
      var line = _buffer;
      _buffer = '';
      if (line.endsWith('\r')) line = line.substring(0, line.length - 1);
      final event = _consumeLine(line);
      if (event != null) events.add(event);
    }
    final event = _flush();
    if (event != null) events.add(event);
    return events;
  }

  MikuEvent? _consumeLine(String line) {
    if (line.isEmpty) return _flush();
    if (line.startsWith(':')) return null;

    final colon = line.indexOf(':');
    final field = colon < 0 ? line : line.substring(0, colon);
    var value = colon < 0 ? '' : line.substring(colon + 1);
    if (value.startsWith(' ')) value = value.substring(1);
    switch (field) {
      case 'event':
        _eventName = value.isEmpty ? 'message' : value;
      case 'id':
        _id = value;
      case 'data':
        _dataLines.add(value);
    }
    return null;
  }

  MikuEvent? _flush() {
    if (_dataLines.isEmpty) {
      _resetFrame();
      return null;
    }

    final eventName = _eventName;
    final id = _id;
    final dataText = _dataLines.join('\n');
    _resetFrame();

    if (eventName != sessionEventSseName) {
      throw FormatException('unsupported SSE event name: $eventName');
    }
    if (id == null || !RegExp(r'^[0-9]+$').hasMatch(id)) {
      throw const FormatException('session event id must be numeric');
    }
    final sequence = int.tryParse(id);
    if (sequence == null || sequence <= 0) {
      throw const FormatException('session event id is out of range');
    }

    final decoded = jsonDecode(dataText);
    if (decoded is! Map) {
      throw const FormatException('session event envelope must be an object');
    }
    final envelope = decoded.cast<String, Object?>();
    final type = envelope['type'];
    final turnId = envelope['turnId'];
    final createdAt = envelope['createdAt'];
    if (type is! String || type.isEmpty) {
      throw const FormatException('session event type is missing');
    }
    if (turnId != null && turnId is! String) {
      throw const FormatException(
        'session event turnId must be null or a string',
      );
    }
    if (createdAt is! String || createdAt.isEmpty) {
      throw const FormatException('session event createdAt is missing');
    }
    final payload = envelope['payload'];
    final data =
        payload is Map
            ? payload.cast<String, Object?>()
            : <String, Object?>{'value': payload};
    return MikuEvent(
      type: type,
      id: id,
      data: data,
      turnId: turnId as String?,
      createdAt: createdAt,
    );
  }

  void _resetFrame() {
    _eventName = 'message';
    _id = null;
    _dataLines.clear();
  }
}

class NumericEventDeduplicator {
  NumericEventDeduplicator(String? initialId)
    : _highWater = numericEventId(initialId) ?? 0;

  int _highWater;

  int get highWater => _highWater;

  bool accept(MikuEvent event) {
    final sequence = numericEventId(event.id);
    if (sequence == null || sequence <= _highWater) return false;
    _highWater = sequence;
    return true;
  }
}

/// Tracks the durable event cursor and makes `session_end` a one-way stream boundary.
///
/// Both native and browser transports use this state so they cannot reconnect after a terminal
/// event or deliver a corrupt post-terminal row that happened to share the final network chunk.
class SessionEventLifecycle {
  SessionEventLifecycle(String? initialId)
    : _deduplicator = NumericEventDeduplicator(initialId);

  final NumericEventDeduplicator _deduplicator;
  bool _isTerminal = false;

  bool get isTerminal => _isTerminal;

  bool get shouldReconnect => !_isTerminal;

  bool accept(MikuEvent event) {
    if (_isTerminal || !_deduplicator.accept(event)) return false;
    if (event.type == 'session_end') _isTerminal = true;
    return true;
  }
}

int? numericEventId(String? value) {
  if (value == null || !RegExp(r'^[0-9]+$').hasMatch(value)) return null;
  final parsed = int.tryParse(value);
  return parsed != null && parsed > 0 ? parsed : null;
}
