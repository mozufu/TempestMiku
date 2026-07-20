part of 'conversation_screen.dart';

class _ProjectPromotionDraft {
  const _ProjectPromotionDraft({
    required this.summary,
    required this.openLoops,
    required this.decisions,
    required this.resources,
  });

  final String? summary;
  final List<String> openLoops;
  final List<String> decisions;
  final List<String> resources;
}

class _ProjectPromotionSheet extends StatefulWidget {
  const _ProjectPromotionSheet({
    required this.projectId,
    required this.suggestedSummary,
  });

  final String projectId;
  final String suggestedSummary;

  @override
  State<_ProjectPromotionSheet> createState() => _ProjectPromotionSheetState();
}

class _ProjectPromotionSheetState extends State<_ProjectPromotionSheet> {
  late final TextEditingController _summaryController;
  final _openLoopsController = TextEditingController();
  final _decisionsController = TextEditingController();
  final _resourcesController = TextEditingController();

  @override
  void initState() {
    super.initState();
    _summaryController = TextEditingController(text: widget.suggestedSummary);
    for (final controller in _controllers) {
      controller.addListener(_changed);
    }
  }

  List<TextEditingController> get _controllers => [
    _summaryController,
    _openLoopsController,
    _decisionsController,
    _resourcesController,
  ];

  @override
  void dispose() {
    for (final controller in _controllers) {
      controller
        ..removeListener(_changed)
        ..dispose();
    }
    super.dispose();
  }

  void _changed() => setState(() {});

  bool get _hasContent =>
      _controllers.any((controller) => controller.text.trim().isNotEmpty);

  void _submit() {
    final summary = _summaryController.text.trim();
    Navigator.of(context).pop(
      _ProjectPromotionDraft(
        summary: summary.isEmpty ? null : summary,
        openLoops: _nonEmptyLines(_openLoopsController.text),
        decisions: _nonEmptyLines(_decisionsController.text),
        resources: _nonEmptyLines(_resourcesController.text),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return FractionallySizedBox(
      heightFactor: 0.92,
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          20,
          4,
          20,
          20 + MediaQuery.viewInsetsOf(context).bottom,
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              '整理到 ${widget.projectId}',
              key: const Key('promotion-title'),
              style: Theme.of(
                context,
              ).textTheme.titleLarge?.copyWith(fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 6),
            Text(
              '先檢查要保留的內容。送出後會寫入 Project 記錄，不會改動原始對話或檔案。',
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
            const SizedBox(height: 14),
            Expanded(
              child: ListView(
                key: const Key('promotion-form'),
                children: [
                  TextField(
                    key: const Key('promotion-summary'),
                    controller: _summaryController,
                    minLines: 3,
                    maxLines: 7,
                    decoration: const InputDecoration(
                      labelText: '摘要',
                      alignLabelWithHint: true,
                      hintText: '這段對話值得保留的結論',
                    ),
                  ),
                  const SizedBox(height: 14),
                  _PromotionLineField(
                    fieldKey: const Key('promotion-open-loops'),
                    controller: _openLoopsController,
                    label: 'Open loops',
                    hint: '每行一項尚未完成的工作',
                  ),
                  const SizedBox(height: 14),
                  _PromotionLineField(
                    fieldKey: const Key('promotion-decisions'),
                    controller: _decisionsController,
                    label: 'Decisions',
                    hint: '每行一項已做出的決定',
                  ),
                  const SizedBox(height: 14),
                  _PromotionLineField(
                    fieldKey: const Key('promotion-resources'),
                    controller: _resourcesController,
                    label: 'Resources',
                    hint: '每行一個 artifact://、workspace:// 或 linked:// URI',
                    monospace: true,
                  ),
                ],
              ),
            ),
            const SizedBox(height: 14),
            Row(
              children: [
                Expanded(
                  child: OutlinedButton(
                    onPressed: () => Navigator.of(context).pop(),
                    child: const Text('取消'),
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: FilledButton.icon(
                    key: const Key('confirm-promotion'),
                    onPressed: _hasContent ? _submit : null,
                    icon: const Icon(Icons.playlist_add_check_rounded),
                    label: const Text('整理到 Project'),
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

class _PromotionLineField extends StatelessWidget {
  const _PromotionLineField({
    required this.fieldKey,
    required this.controller,
    required this.label,
    required this.hint,
    this.monospace = false,
  });

  final Key fieldKey;
  final TextEditingController controller;
  final String label;
  final String hint;
  final bool monospace;

  @override
  Widget build(BuildContext context) {
    return TextField(
      key: fieldKey,
      controller: controller,
      minLines: 2,
      maxLines: 5,
      style: monospace ? const TextStyle(fontFamily: 'monospace') : null,
      decoration: InputDecoration(
        labelText: label,
        alignLabelWithHint: true,
        hintText: hint,
      ),
    );
  }
}

List<String> _nonEmptyLines(String value) {
  return value
      .split('\n')
      .map((line) => line.trim())
      .where((line) => line.isNotEmpty)
      .toList();
}
