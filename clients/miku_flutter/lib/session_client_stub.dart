import 'dart:async';

import 'session_models.dart';

MikuSessionClient createClient() => ScriptedMikuClient();

class ScriptedMikuClient implements MikuSessionClient {
  final Map<String, StreamController<MikuEvent>> _controllers = {};
  int _nextId = 0;

  @override
  Future<MikuSession> createSession() async {
    final id = 'scripted-${_nextId++}';
    _controllers[id] = StreamController<MikuEvent>.broadcast();
    return MikuSession(
      id: id,
      mode: 'personal_assistant',
      label: 'Personal Assistant',
      voiceCap: '中',
    );
  }

  @override
  Stream<MikuEvent> events(String sessionId, {String? lastEventId}) {
    return _controllers
        .putIfAbsent(sessionId, () => StreamController<MikuEvent>.broadcast())
        .stream;
  }

  @override
  Future<void> sendMessage(String sessionId, String content) async {
    final controller = _controllers[sessionId];
    if (controller == null) return;
    if (content.toLowerCase().contains('code')) {
      controller.add(
        const MikuEvent(
          type: 'mode',
          id: '2',
          data: {
            'mode': 'serious_engineer',
            'label': 'Serious Engineer',
            'voice_cap': '關',
          },
        ),
      );
    }
    final text = 'Miku heard: $content';
    controller.add(MikuEvent(type: 'text', id: '3', data: {'delta': text}));
    controller.add(MikuEvent(type: 'final', id: '4', data: {'text': text}));
  }

  @override
  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision,
  ) async {}

  @override
  Future<void> lockMode(String sessionId, String mode) async {
    _controllers[sessionId]?.add(
      MikuEvent(
        type: 'mode',
        data: {
          'mode': mode,
          'label': _label(mode),
          'lock_source': 'user',
        },
      ),
    );
  }

  @override
  Future<void> unlockMode(String sessionId) async {
    _controllers[sessionId]?.add(
      const MikuEvent(
        type: 'mode',
        data: {'mode': 'personal_assistant', 'label': 'Personal Assistant'},
      ),
    );
  }

  @override
  Future<ProjectOverview> projectOverview(String sessionId) async {
    return const ProjectOverview(
      status: 'Scripted project status',
      nextActions: ['Continue from latest session result'],
    );
  }

  String _label(String mode) {
    return switch (mode) {
      'serious_engineer' => 'Serious Engineer',
      'handoff' => 'Handoff',
      'ambiguity_grill' => 'Ambiguity Grill',
      'negative_state_grounding' => 'Negative-State Grounding',
      _ => 'Personal Assistant',
    };
  }
}
