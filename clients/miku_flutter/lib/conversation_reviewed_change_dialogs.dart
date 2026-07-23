part of 'conversation_screen.dart';

class _ReviewedChangesSheet extends StatelessWidget {
  const _ReviewedChangesSheet({
    required this.catalog,
    required this.currentModeId,
  });

  final ModeCatalog? catalog;
  final String currentModeId;

  Future<void> _pick(
    BuildContext context,
    Future<_ReviewedChangeRequest?> Function() open,
  ) async {
    final request = await open();
    if (request != null && context.mounted) {
      Navigator.of(context).pop(request);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    return FractionallySizedBox(
      key: const Key('reviewed-changes-sheet'),
      heightFactor: 0.82,
      child: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 680),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(20, 4, 20, 24),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        '經審核的變更',
                        key: const Key('reviewed-changes-title'),
                        style: Theme.of(context).textTheme.titleLarge?.copyWith(
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    IconButton(
                      tooltip: '關閉',
                      onPressed: () => Navigator.of(context).pop(),
                      icon: const Icon(Icons.close_rounded),
                    ),
                  ],
                ),
                Text(
                  '這裡只建立有界提案。記憶、指引與版本回溯 都必須回到對話中手動核准，才可能寫入或啟用。',
                  style: Theme.of(
                    context,
                  ).textTheme.bodySmall?.copyWith(color: palette.muted),
                ),
                const SizedBox(height: 18),
                Expanded(
                  child: ListView(
                    children: [
                      _ReviewedChangeTile(
                        key: const Key('propose-memory-change'),
                        icon: Icons.psychology_outlined,
                        title: '提出記憶',
                        subtitle: '個人偏好／事實，或目前範圍的可回想片段',
                        onTap:
                            () => _pick(
                              context,
                              () => showDialog<_ReviewedChangeRequest>(
                                context: context,
                                builder:
                                    (context) => const _MemoryProposalDialog(),
                              ),
                            ),
                      ),
                      const SizedBox(height: 10),
                      _ReviewedChangeTile(
                        key: const Key('propose-guidance-change'),
                        icon: Icons.tune_rounded,
                        title: '提出指引變更',
                        subtitle: '只提交結構化摘要，不接受原始提示詞或修改片段',
                        onTap:
                            () => _pick(
                              context,
                              () => showDialog<_ReviewedChangeRequest>(
                                context: context,
                                builder:
                                    (context) => _EvolutionProposalDialog(
                                      catalog: catalog,
                                      currentModeId: currentModeId,
                                    ),
                              ),
                            ),
                      ),
                      const SizedBox(height: 10),
                      _ReviewedChangeTile(
                        key: const Key('propose-version-rollback'),
                        icon: Icons.history_toggle_off_rounded,
                        title: '提出版本回溯',
                        subtitle: '以完整 SHA-256 digest 鎖定目前與目標版本',
                        onTap:
                            () => _pick(
                              context,
                              () => showDialog<_ReviewedChangeRequest>(
                                context: context,
                                builder:
                                    (context) => _RollbackProposalDialog(
                                      catalog: catalog,
                                      currentModeId: currentModeId,
                                    ),
                              ),
                            ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _ReviewedChangeTile extends StatelessWidget {
  const _ReviewedChangeTile({
    required super.key,
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.onTap,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Card.outlined(
      margin: EdgeInsets.zero,
      child: ListTile(
        minTileHeight: 72,
        leading: Icon(icon),
        title: Text(title),
        subtitle: Text(subtitle),
        trailing: const Icon(Icons.chevron_right_rounded),
        onTap: onTap,
      ),
    );
  }
}

enum _MemoryProposalKind { profileFact, recallChunk }

class _MemoryProposalDialog extends StatefulWidget {
  const _MemoryProposalDialog();

  @override
  State<_MemoryProposalDialog> createState() => _MemoryProposalDialogState();
}

class _MemoryProposalDialogState extends State<_MemoryProposalDialog> {
  final _formKey = GlobalKey<FormState>();
  final _predicate = TextEditingController();
  final _object = TextEditingController();
  final _text = TextEditingController();
  _MemoryProposalKind _kind = _MemoryProposalKind.profileFact;

  @override
  void dispose() {
    _predicate.dispose();
    _object.dispose();
    _text.dispose();
    super.dispose();
  }

  void _submit() {
    if (!(_formKey.currentState?.validate() ?? false)) return;
    final request = switch (_kind) {
      _MemoryProposalKind.profileFact => MemoryWriteProposalRequest.profileFact(
        predicate: _predicate.text.trim(),
        object: _object.text.trim(),
      ),
      _MemoryProposalKind.recallChunk => MemoryWriteProposalRequest.recallChunk(
        text: _text.text.trim(),
      ),
    };
    Navigator.of(context).pop(_ReviewedMemoryRequest(request));
  }

  String? _required(String? value) =>
      value?.trim().isEmpty ?? true ? '這個欄位不能留白。' : null;

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('提出記憶'),
      content: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560),
        child: Form(
          key: _formKey,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                DropdownButtonFormField<_MemoryProposalKind>(
                  key: const Key('memory-proposal-kind'),
                  isExpanded: true,
                  initialValue: _kind,
                  decoration: const InputDecoration(
                    labelText: '記憶類型',
                    border: OutlineInputBorder(),
                  ),
                  items: const [
                    DropdownMenuItem(
                      value: _MemoryProposalKind.profileFact,
                      child: Text('個人偏好／事實'),
                    ),
                    DropdownMenuItem(
                      value: _MemoryProposalKind.recallChunk,
                      child: Text('可回想片段'),
                    ),
                  ],
                  onChanged:
                      (value) => setState(
                        () => _kind = value ?? _MemoryProposalKind.profileFact,
                      ),
                ),
                const SizedBox(height: 14),
                if (_kind == _MemoryProposalKind.profileFact) ...[
                  TextFormField(
                    key: const Key('memory-predicate'),
                    controller: _predicate,
                    maxLength: 160,
                    validator: _required,
                    decoration: const InputDecoration(
                      labelText: '關係／屬性',
                      hintText: '例如：偏好介面語言',
                      helperText: '描述這個事實是哪一種關係。',
                      border: OutlineInputBorder(),
                    ),
                  ),
                  const SizedBox(height: 10),
                  TextFormField(
                    key: const Key('memory-object'),
                    controller: _object,
                    maxLength: 2000,
                    minLines: 2,
                    maxLines: 5,
                    validator: _required,
                    decoration: const InputDecoration(
                      labelText: '內容',
                      hintText: '例如：繁體中文',
                      border: OutlineInputBorder(),
                    ),
                  ),
                ] else
                  TextFormField(
                    key: const Key('memory-recall-text'),
                    controller: _text,
                    maxLength: 4000,
                    minLines: 4,
                    maxLines: 8,
                    validator: _required,
                    decoration: const InputDecoration(
                      labelText: '要保留的片段',
                      helperText: '會寫入這段對話目前的記憶範圍。',
                      border: OutlineInputBorder(),
                    ),
                  ),
                const SizedBox(height: 4),
                Text(
                  '請勿放入 token、密碼或私鑰；伺服器會拒絕偵測到的敏感資料。',
                  style: Theme.of(context).textTheme.bodySmall?.copyWith(
                    color: TmTokens.of(context).muted,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(
          key: const Key('submit-memory-proposal'),
          onPressed: _submit,
          child: const Text('建立待確認提案'),
        ),
      ],
    );
  }
}

enum _EvolutionTargetKind { persona, mode }

class _EvolutionProposalDialog extends StatefulWidget {
  const _EvolutionProposalDialog({
    required this.catalog,
    required this.currentModeId,
  });

  final ModeCatalog? catalog;
  final String currentModeId;

  @override
  State<_EvolutionProposalDialog> createState() =>
      _EvolutionProposalDialogState();
}

class _EvolutionProposalDialogState extends State<_EvolutionProposalDialog> {
  final _formKey = GlobalKey<FormState>();
  final _modeId = TextEditingController();
  final _label = TextEditingController();
  final _summary = TextEditingController();
  _EvolutionTargetKind _kind = _EvolutionTargetKind.persona;
  String _section = 'tone_guidance';

  @override
  void initState() {
    super.initState();
    final modes = widget.catalog?.modes ?? const <ModeProfile>[];
    _modeId.text =
        modes.any((mode) => mode.id == widget.currentModeId)
            ? widget.currentModeId
            : modes.isEmpty
            ? widget.currentModeId
            : modes.first.id;
  }

  @override
  void dispose() {
    _modeId.dispose();
    _label.dispose();
    _summary.dispose();
    super.dispose();
  }

  List<({String value, String label})> get _sections =>
      _kind == _EvolutionTargetKind.persona
          ? const [
            (value: 'tone_guidance', label: '語氣指引'),
            (value: 'address_guidance', label: '稱呼指引'),
            (value: 'interaction_preference', label: '互動偏好'),
          ]
          : const [
            (value: 'description', label: '模式描述'),
            (value: 'routing_guidance', label: '路由指引'),
          ];

  void _changeKind(_EvolutionTargetKind? value) {
    if (value == null) return;
    setState(() {
      _kind = value;
      _section =
          value == _EvolutionTargetKind.persona
              ? 'tone_guidance'
              : 'description';
    });
  }

  String? _required(String? value) =>
      value?.trim().isEmpty ?? true ? '這個欄位不能留白。' : null;

  void _submit() {
    if (!(_formKey.currentState?.validate() ?? false)) return;
    final target =
        _kind == _EvolutionTargetKind.persona
            ? const EvolutionReviewTarget.persona('miku')
            : EvolutionReviewTarget.mode(_modeId.text.trim());
    Navigator.of(context).pop(
      _ReviewedEvolutionRequest(
        EvolutionReviewProposalRequest(
          target: target,
          changes: [
            EvolutionReviewChange(
              section: _section,
              after: EvolutionReviewMetadata(
                label: _label.text.trim(),
                summary: _summary.text.trim(),
              ),
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final modes = widget.catalog?.modes ?? const <ModeProfile>[];
    return AlertDialog(
      title: const Text('提出指引變更'),
      content: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560),
        child: Form(
          key: _formKey,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                DropdownButtonFormField<_EvolutionTargetKind>(
                  key: const Key('evolution-target-kind'),
                  isExpanded: true,
                  initialValue: _kind,
                  decoration: const InputDecoration(
                    labelText: '目標',
                    border: OutlineInputBorder(),
                  ),
                  items: const [
                    DropdownMenuItem(
                      value: _EvolutionTargetKind.persona,
                      child: Text('Miku persona'),
                    ),
                    DropdownMenuItem(
                      value: _EvolutionTargetKind.mode,
                      child: Text('模式'),
                    ),
                  ],
                  onChanged: _changeKind,
                ),
                if (_kind == _EvolutionTargetKind.mode) ...[
                  const SizedBox(height: 12),
                  if (modes.isNotEmpty)
                    DropdownButtonFormField<String>(
                      key: const Key('evolution-mode-id'),
                      isExpanded: true,
                      initialValue:
                          modes.any((mode) => mode.id == _modeId.text)
                              ? _modeId.text
                              : modes.first.id,
                      decoration: const InputDecoration(
                        labelText: '模式',
                        border: OutlineInputBorder(),
                      ),
                      items: [
                        for (final mode in modes)
                          DropdownMenuItem(
                            value: mode.id,
                            child: Text(mode.label),
                          ),
                      ],
                      onChanged: (value) => _modeId.text = value ?? '',
                    )
                  else
                    TextFormField(
                      key: const Key('evolution-mode-id'),
                      controller: _modeId,
                      validator: _required,
                      decoration: const InputDecoration(
                        labelText: '模式代號',
                        border: OutlineInputBorder(),
                      ),
                    ),
                ],
                const SizedBox(height: 12),
                DropdownButtonFormField<String>(
                  key: const Key('evolution-section'),
                  isExpanded: true,
                  initialValue: _section,
                  decoration: const InputDecoration(
                    labelText: '變更區段',
                    border: OutlineInputBorder(),
                  ),
                  items: [
                    for (final section in _sections)
                      DropdownMenuItem(
                        value: section.value,
                        child: Text(section.label),
                      ),
                  ],
                  onChanged:
                      (value) => setState(() => _section = value ?? _section),
                ),
                const SizedBox(height: 12),
                TextFormField(
                  key: const Key('evolution-change-label'),
                  controller: _label,
                  maxLength: 160,
                  validator: _required,
                  decoration: const InputDecoration(
                    labelText: '變更標題',
                    border: OutlineInputBorder(),
                  ),
                ),
                const SizedBox(height: 8),
                TextFormField(
                  key: const Key('evolution-change-summary'),
                  controller: _summary,
                  maxLength: 2000,
                  minLines: 3,
                  maxLines: 7,
                  validator: _required,
                  decoration: const InputDecoration(
                    labelText: '有界摘要',
                    helperText: '這是待審核的變更摘要，不是系統提示詞本身。',
                    border: OutlineInputBorder(),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(
          key: const Key('submit-evolution-proposal'),
          onPressed: _submit,
          child: const Text('建立待確認提案'),
        ),
      ],
    );
  }
}

class _RollbackProposalDialog extends StatefulWidget {
  const _RollbackProposalDialog({
    required this.catalog,
    required this.currentModeId,
  });

  final ModeCatalog? catalog;
  final String currentModeId;

  @override
  State<_RollbackProposalDialog> createState() =>
      _RollbackProposalDialogState();
}

class _RollbackProposalDialogState extends State<_RollbackProposalDialog> {
  final _formKey = GlobalKey<FormState>();
  final _targetId = TextEditingController();
  final _expectedDigest = TextEditingController();
  final _targetDigest = TextEditingController();
  _RollbackTargetKind _kind = _RollbackTargetKind.mode;
  bool _returnToBase = false;

  @override
  void initState() {
    super.initState();
    final modes = widget.catalog?.modes ?? const <ModeProfile>[];
    _targetId.text =
        modes.any((mode) => mode.id == widget.currentModeId)
            ? widget.currentModeId
            : modes.isEmpty
            ? widget.currentModeId
            : modes.first.id;
  }

  @override
  void dispose() {
    _targetId.dispose();
    _expectedDigest.dispose();
    _targetDigest.dispose();
    super.dispose();
  }

  String? _required(String? value) =>
      value?.trim().isEmpty ?? true ? '這個欄位不能留白。' : null;

  String? _digest(String? value) {
    final required = _required(value);
    if (required != null) return required;
    if (!RegExp(r'^sha256:[0-9a-f]{64}$').hasMatch(value!.trim())) {
      return '請輸入完整的 sha256: 加 64 位小寫十六進位 digest。';
    }
    return null;
  }

  Future<void> _pasteInto(TextEditingController controller) async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text?.trim();
    if (text == null || text.isEmpty) return;
    controller.text = text;
  }

  void _changeKind(_RollbackTargetKind? value) {
    if (value == null) return;
    setState(() {
      _kind = value;
      _returnToBase = false;
      _targetId.text = switch (value) {
        _RollbackTargetKind.mode => widget.currentModeId,
        _RollbackTargetKind.persona => 'miku',
        _RollbackTargetKind.skill => '',
      };
    });
  }

  Future<void> _submit() async {
    if (!(_formKey.currentState?.validate() ?? false)) return;
    final targetDigest = _returnToBase ? null : _targetDigest.text.trim();
    final request = _ReviewedRollbackRequest(
      kind: _kind,
      targetId: _targetId.text.trim(),
      expectedActiveDigest: _expectedDigest.text.trim(),
      targetDigest: targetDigest,
    );
    final confirmed = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder:
          (context) => AlertDialog(
            title: const Text('核對版本回溯'),
            content: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 520),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _RollbackFact(label: '類型', value: _kind.name),
                  _RollbackFact(label: '目標', value: request.targetId),
                  _RollbackFact(
                    label: '目前版本',
                    value: request.expectedActiveDigest,
                  ),
                  _RollbackFact(
                    label: '切換到',
                    value: request.targetDigest ?? '預設版本（停用補充內容）',
                  ),
                  const SizedBox(height: 12),
                  const Text('送出後仍須在對話中再次核准；這一步不會直接切換版本。'),
                ],
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(false),
                child: const Text('返回修改'),
              ),
              FilledButton(
                key: const Key('confirm-rollback-proposal'),
                onPressed: () => Navigator.of(context).pop(true),
                child: const Text('建立待確認提案'),
              ),
            ],
          ),
    );
    if (confirmed == true && mounted) Navigator.of(context).pop(request);
  }

  @override
  Widget build(BuildContext context) {
    final modes = widget.catalog?.modes ?? const <ModeProfile>[];
    return AlertDialog(
      title: const Text('提出版本回溯'),
      content: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 580),
        child: Form(
          key: _formKey,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                DropdownButtonFormField<_RollbackTargetKind>(
                  key: const Key('rollback-target-kind'),
                  isExpanded: true,
                  initialValue: _kind,
                  decoration: const InputDecoration(
                    labelText: '版本類型',
                    border: OutlineInputBorder(),
                  ),
                  items: const [
                    DropdownMenuItem(
                      value: _RollbackTargetKind.mode,
                      child: Text('模式補充內容'),
                    ),
                    DropdownMenuItem(
                      value: _RollbackTargetKind.persona,
                      child: Text('人格補充內容'),
                    ),
                    DropdownMenuItem(
                      value: _RollbackTargetKind.skill,
                      child: Text('受管理技能'),
                    ),
                  ],
                  onChanged: _changeKind,
                ),
                const SizedBox(height: 12),
                if (_kind == _RollbackTargetKind.mode && modes.isNotEmpty)
                  DropdownButtonFormField<String>(
                    key: const Key('rollback-target-id'),
                    isExpanded: true,
                    initialValue:
                        modes.any((mode) => mode.id == _targetId.text)
                            ? _targetId.text
                            : modes.first.id,
                    decoration: const InputDecoration(
                      labelText: '模式',
                      border: OutlineInputBorder(),
                    ),
                    items: [
                      for (final mode in modes)
                        DropdownMenuItem(
                          value: mode.id,
                          child: Text(mode.label),
                        ),
                    ],
                    onChanged: (value) => _targetId.text = value ?? '',
                  )
                else
                  TextFormField(
                    key: const Key('rollback-target-id'),
                    controller: _targetId,
                    enabled: _kind != _RollbackTargetKind.persona,
                    validator: _required,
                    decoration: InputDecoration(
                      labelText:
                          _kind == _RollbackTargetKind.skill
                              ? '技能名稱'
                              : _kind == _RollbackTargetKind.persona
                              ? '人格代號'
                              : '模式代號',
                      border: const OutlineInputBorder(),
                    ),
                  ),
                const SizedBox(height: 12),
                TextFormField(
                  key: const Key('rollback-expected-digest'),
                  controller: _expectedDigest,
                  autocorrect: false,
                  enableSuggestions: false,
                  validator: _digest,
                  style: const TextStyle(fontFamily: 'monospace'),
                  decoration: InputDecoration(
                    labelText: '目前啟用的版本 digest',
                    helperText:
                        '必須與伺服器目前版本完全相同，否則會直接拒絕執行。'
                        '可在資源檢視器複製版本 digest 後貼上。',
                    border: const OutlineInputBorder(),
                    suffixIcon: IconButton(
                      key: const Key('rollback-expected-digest-paste'),
                      icon: const Icon(Icons.content_paste_rounded),
                      tooltip: '貼上',
                      onPressed: () => _pasteInto(_expectedDigest),
                    ),
                  ),
                ),
                if (_kind != _RollbackTargetKind.skill) ...[
                  const SizedBox(height: 4),
                  SwitchListTile.adaptive(
                    key: const Key('rollback-return-to-base'),
                    contentPadding: EdgeInsets.zero,
                    value: _returnToBase,
                    onChanged: (value) => setState(() => _returnToBase = value),
                    title: const Text('回復為預設版本'),
                    subtitle: const Text('停用目前的補充內容，不會刪除既有版本。'),
                  ),
                ],
                if (!_returnToBase) ...[
                  const SizedBox(height: 8),
                  TextFormField(
                    key: const Key('rollback-target-digest'),
                    controller: _targetDigest,
                    autocorrect: false,
                    enableSuggestions: false,
                    validator: _digest,
                    style: const TextStyle(fontFamily: 'monospace'),
                    decoration: InputDecoration(
                      labelText: '目標版本 digest',
                      helperText: '可在資源檢視器複製版本 digest 後貼上。',
                      border: const OutlineInputBorder(),
                      suffixIcon: IconButton(
                        key: const Key('rollback-target-digest-paste'),
                        icon: const Icon(Icons.content_paste_rounded),
                        tooltip: '貼上',
                        onPressed: () => _pasteInto(_targetDigest),
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(
          key: const Key('review-rollback-proposal'),
          onPressed: _submit,
          child: const Text('核對版本'),
        ),
      ],
    );
  }
}

class _RollbackFact extends StatelessWidget {
  const _RollbackFact({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 5),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: Theme.of(context).textTheme.labelSmall),
          const SizedBox(height: 2),
          SelectableText(
            value,
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(fontFamily: 'monospace'),
          ),
        ],
      ),
    );
  }
}
