import 'dart:async';

import 'package:flutter/material.dart';

import 'session_client.dart';
import 'session_models.dart';

void main() {
  runApp(MikuApp(client: createDefaultClient()));
}

class MikuApp extends StatelessWidget {
  const MikuApp({super.key, required this.client});

  final MikuSessionClient client;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'TempestMiku',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xFF2563EB)),
        useMaterial3: true,
      ),
      home: MikuHomePage(client: client),
    );
  }
}

class MikuHomePage extends StatefulWidget {
  const MikuHomePage({super.key, required this.client});

  final MikuSessionClient client;

  @override
  State<MikuHomePage> createState() => _MikuHomePageState();
}

class _MikuHomePageState extends State<MikuHomePage> {
  final TextEditingController _message = TextEditingController();
  final List<ApprovalPrompt> _approvals = [];
  final List<String> _nextActions = [];
  StreamSubscription<MikuEvent>? _events;
  String? _sessionId;
  String? _lastEventId;
  String _mode = 'personal_assistant';
  String _modeLabel = 'Personal Assistant';
  String _status = 'idle';
  String _streamText = '';
  String _finalText = '';
  String _projectStatus = '';

  @override
  void dispose() {
    _message.dispose();
    _events?.cancel();
    super.dispose();
  }

  Future<void> _ensureSession() async {
    if (_sessionId != null) return;
    final session = await widget.client.createSession();
    _sessionId = session.id;
    _mode = session.mode;
    _modeLabel = session.label;
    _status = 'connected';
    _events = widget.client
        .events(session.id, lastEventId: _lastEventId)
        .listen(_handleEvent, onError: (_) {
      if (mounted) setState(() => _status = 'reconnecting');
    });
    if (mounted) setState(() {});
  }

  void _handleEvent(MikuEvent event) {
    if (event.id != null) _lastEventId = event.id;
    setState(() {
      switch (event.type) {
        case 'text':
          _streamText += event.data['delta'] as String? ?? '';
          break;
        case 'final':
          _finalText = event.data['text'] as String? ?? '';
          _status = 'complete';
          _loadProject();
          break;
        case 'mode':
          _mode = event.data['mode'] as String? ?? _mode;
          _modeLabel = event.data['label'] as String? ?? _modeLabel;
          break;
        case 'approval':
          _approvals.add(
            ApprovalPrompt(
              approvalId: event.data['approvalId'] as String? ?? '',
              action: event.data['action'] as String? ?? 'Approval requested',
              scope: (event.data['scope'] as Map?)?.cast<String, Object?>() ??
                  const <String, Object?>{},
            ),
          );
          break;
        case 'approval_resolved':
          _approvals.removeWhere(
            (approval) => approval.approvalId == event.data['approvalId'],
          );
          break;
        default:
          break;
      }
    });
  }

  Future<void> _send() async {
    final content = _message.text.trim();
    if (content.isEmpty) return;
    await _ensureSession();
    setState(() {
      _status = 'streaming';
      _streamText = '';
      _finalText = '';
    });
    await widget.client.sendMessage(_sessionId!, content);
    _message.clear();
  }

  Future<void> _lockMode() async {
    await _ensureSession();
    await widget.client.lockMode(_sessionId!, _mode);
  }

  Future<void> _unlockMode() async {
    await _ensureSession();
    await widget.client.unlockMode(_sessionId!);
  }

  Future<void> _resolve(ApprovalPrompt approval, String decision) async {
    await widget.client.resolveApproval(
      _sessionId!,
      approval.approvalId,
      decision,
    );
    setState(() => _approvals.remove(approval));
  }

  Future<void> _loadProject() async {
    final id = _sessionId;
    if (id == null) return;
    final overview = await widget.client.projectOverview(id);
    if (!mounted) return;
    setState(() {
      _projectStatus = overview.status;
      _nextActions
        ..clear()
        ..addAll(overview.nextActions);
    });
  }

  Future<void> _openResource(String uri) async {
    await _ensureSession();
    setState(() => _status = 'opening resource');
    try {
      final preview = await widget.client.previewResource(_sessionId!, uri);
      if (!mounted) return;
      setState(() => _status = 'connected');
      await showModalBottomSheet<void>(
        context: context,
        showDragHandle: true,
        isScrollControlled: true,
        builder: (context) => _ResourcePreviewSheet(preview: preview),
      );
    } catch (error) {
      if (!mounted) return;
      setState(() => _status = 'resource error');
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not open $uri: $error')),
      );
    }
  }

  Future<void> _promoteSession() async {
    await _ensureSession();
    final resources = _resourceUris('$_streamText\n$_finalText');
    final summary =
        (_finalText.trim().isNotEmpty ? _finalText : _streamText).trim();
    setState(() => _status = 'promoting');
    try {
      final promotion = await widget.client.promoteSession(
        _sessionId!,
        summary: summary.isEmpty ? null : summary,
        resources: resources,
      );
      if (!mounted) return;
      setState(() {
        _status = 'promoted';
        _projectStatus =
            '${promotion.projectUri} - ${promotion.promotedCount} promoted';
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _status = 'promotion error');
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not promote session: $error')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final resources = _resourceUris('$_streamText\n$_finalText');
    return Scaffold(
      appBar: AppBar(
        title: const Text('TempestMiku'),
        actions: [
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: Center(child: _Badge(label: _modeLabel)),
          ),
        ],
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(16),
          children: [
            Wrap(
              spacing: 8,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                Chip(label: Text(_status)),
                DropdownButton<String>(
                  value: _mode,
                  items: const [
                    DropdownMenuItem(
                      value: 'personal_assistant',
                      child: Text('Personal Assistant'),
                    ),
                    DropdownMenuItem(
                      value: 'ambiguity_grill',
                      child: Text('Ambiguity Grill'),
                    ),
                    DropdownMenuItem(
                      value: 'negative_state_grounding',
                      child: Text('Negative-State Grounding'),
                    ),
                    DropdownMenuItem(
                      value: 'serious_engineer',
                      child: Text('Serious Engineer'),
                    ),
                    DropdownMenuItem(value: 'handoff', child: Text('Handoff')),
                  ],
                  onChanged: (value) => setState(() {
                    if (value != null) _mode = value;
                  }),
                ),
                IconButton(
                  tooltip: 'Lock mode',
                  onPressed: _lockMode,
                  icon: const Icon(Icons.lock_outline),
                ),
                IconButton(
                  tooltip: 'Unlock mode',
                  onPressed: _unlockMode,
                  icon: const Icon(Icons.lock_open),
                ),
                IconButton(
                  tooltip: 'Refresh project',
                  onPressed: _loadProject,
                  icon: const Icon(Icons.refresh),
                ),
                IconButton(
                  tooltip: 'Promote session',
                  onPressed: _promoteSession,
                  icon: const Icon(Icons.upload_file),
                ),
              ],
            ),
            const SizedBox(height: 12),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _message,
                    decoration: const InputDecoration(
                      border: OutlineInputBorder(),
                      hintText: 'Message Miku',
                    ),
                    onSubmitted: (_) => _send(),
                  ),
                ),
                const SizedBox(width: 8),
                FilledButton(
                  onPressed: _send,
                  child: const Text('Send'),
                ),
              ],
            ),
            const SizedBox(height: 16),
            _Panel(title: 'Stream', text: _streamText),
            const SizedBox(height: 12),
            _Panel(title: 'Final', text: _finalText),
            if (resources.isNotEmpty) ...[
              const SizedBox(height: 12),
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  for (final uri in resources)
                    ActionChip(
                      avatar: const Icon(Icons.link),
                      label: Text(uri),
                      onPressed: () => _openResource(uri),
                    ),
                ],
              ),
            ],
            if (_approvals.isNotEmpty) ...[
              const SizedBox(height: 16),
              for (final approval in _approvals)
                Card(
                  child: ListTile(
                    title: Text(approval.action),
                    subtitle: Text(approval.scope.toString()),
                    trailing: Wrap(
                      spacing: 8,
                      children: [
                        TextButton(
                          onPressed: () => _resolve(approval, 'deny'),
                          child: const Text('Deny'),
                        ),
                        FilledButton(
                          onPressed: () => _resolve(approval, 'approve'),
                          child: const Text('Approve'),
                        ),
                      ],
                    ),
                  ),
                ),
            ],
            const SizedBox(height: 16),
            Text('Project', style: Theme.of(context).textTheme.titleMedium),
            const SizedBox(height: 8),
            Text(_projectStatus),
            for (final action in _nextActions)
              ListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                leading: const Icon(Icons.arrow_right),
                title: Text(action),
              ),
          ],
        ),
      ),
    );
  }

  List<String> _resourceUris(String text) {
    final pattern =
        RegExp(r'\b(?:artifact|workspace|linked|project)://[^\s),\]]+');
    return pattern
        .allMatches(text)
        .map((match) => match.group(0)!.replaceAll(RegExp(r'[.。]+$'), ''))
        .toSet()
        .toList();
  }
}

class _Badge extends StatelessWidget {
  const _Badge({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        border: Border.all(color: Theme.of(context).colorScheme.outline),
        borderRadius: BorderRadius.circular(999),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
        child: Text(label, overflow: TextOverflow.ellipsis),
      ),
    );
  }
}

class _Panel extends StatelessWidget {
  const _Panel({required this.title, required this.text});

  final String title;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(title, style: Theme.of(context).textTheme.titleMedium),
        const SizedBox(height: 8),
        Container(
          width: double.infinity,
          constraints: const BoxConstraints(minHeight: 84),
          padding: const EdgeInsets.all(12),
          decoration: BoxDecoration(
            border: Border.all(color: Theme.of(context).colorScheme.outline),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Text(text),
        ),
      ],
    );
  }
}

class _ResourcePreviewSheet extends StatelessWidget {
  const _ResourcePreviewSheet({required this.preview});

  final ResourcePreview preview;

  @override
  Widget build(BuildContext context) {
    final title =
        preview.title?.isNotEmpty == true ? preview.title! : preview.uri;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(title, style: Theme.of(context).textTheme.titleMedium),
              const SizedBox(height: 6),
              Text(
                '${preview.kind} / ${preview.mime} / ${preview.sizeBytes} bytes',
                style: Theme.of(context).textTheme.bodySmall,
              ),
              const SizedBox(height: 12),
              SelectableText(preview.uri),
              const SizedBox(height: 12),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  border: Border.all(
                    color: Theme.of(context).colorScheme.outline,
                  ),
                  borderRadius: BorderRadius.circular(8),
                ),
                child: SelectableText(
                  preview.preview.isEmpty ? '(empty preview)' : preview.preview,
                ),
              ),
              if (preview.hasMore) ...[
                const SizedBox(height: 8),
                Text(
                  'Preview truncated',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
