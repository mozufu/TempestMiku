import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'session_client.dart';
import 'session_models.dart';

void main() {
  runApp(MikuApp(client: createDefaultClient()));
}

// ─── Design tokens (from inm design system) ──────────────────────────────────
// Light: warm stone canvas. Hex values from colors_and_type.css comments.
// Dark: plum black canvas.

class _Tok {
  final Color bg, surface, raised, border, text, muted;
  final Color accent, accentSoft, cool, onAccent;
  const _Tok({
    required this.bg,
    required this.surface,
    required this.raised,
    required this.border,
    required this.text,
    required this.muted,
    required this.accent,
    required this.accentSoft,
    required this.cool,
    required this.onAccent,
  });

  static const light = _Tok(
    bg: Color(0xFFC9BFB6),
    surface: Color(0xFFDAD2CB),
    raised: Color(0xFFE8E1DB),
    border: Color(0xFFAFA8A3),
    text: Color(0xFF2F2730),
    muted: Color(0xFF625B5E),
    accent: Color(0xFF7E4F49),
    accentSoft: Color(0xFFA1736B),
    cool: Color(0xFF79837F),
    onAccent: Color(0xFFE8E1DB),
  );

  static const dark = _Tok(
    bg: Color(0xFF3E343F),
    surface: Color(0xFF493E4A),
    raised: Color(0xFF584B59),
    border: Color(0xFF6B6069),
    text: Color(0xFFE8E1DB),
    muted: Color(0xFFB6AFAD),
    accent: Color(0xFFD7A095),
    accentSoft: Color(0xFFC58E82),
    cool: Color(0xFFAAB4B0),
    onAccent: Color(0xFF3E343F),
  );
}

// Mode-temperature accent overrides. These are fixed OKLCH values from the design.
Color _modeAccent(String temp, _Tok tok) {
  switch (temp) {
    case 'hot':
      return const Color(
          0xFFB84A30); // oklch(58% 0.094 33) — saturated rust orange
    case 'soft':
      return const Color(0xFFAA7860); // oklch(64% 0.058 28) — warm terracotta
    case 'warm':
      return const Color(0xFFA1736B); // clay-500
    case 'cool':
      return tok.cool;
    default:
      return const Color(0xFFA1736B);
  }
}

// Pick white or dark text for contrast on a given background.
Color _textOn(Color bg) {
  final lum = (0.299 * bg.red + 0.587 * bg.green + 0.114 * bg.blue) / 255;
  return lum > 0.55 ? const Color(0xFF2F2730) : Colors.white;
}

// ─── Mode definitions ─────────────────────────────────────────────────────────

class _Mode {
  final String id, zh, short, cap, temp, tip;
  final double intensity; // 0–100
  final IconData icon;
  const _Mode({
    required this.id,
    required this.zh,
    required this.short,
    required this.cap,
    required this.temp,
    required this.tip,
    required this.intensity,
    required this.icon,
  });

  String get tempLabel => switch (temp) {
        'hot' => '尖銳 · 濃',
        'soft' => '安撫 · 濃',
        'warm' => '親切 · 中',
        'cool' => '克制 · 關',
        _ => '中',
      };
}

const _kModes = <_Mode>[
  _Mode(
    id: 'personal_assistant',
    zh: '個人助理',
    short: '助理',
    cap: '中',
    temp: 'warm',
    intensity: 46,
    tip: '規劃 · 提醒 · 寫作 · 開放迴圈',
    icon: Icons.chat_bubble_outline,
  ),
  _Mode(
    id: 'ambiguity_grill',
    zh: '燒烤我',
    short: '燒烤',
    cap: '濃',
    temp: 'hot',
    intensity: 8,
    tip: '模糊 / 矛盾 → 3–7 個尖銳提問',
    icon: Icons.bolt,
  ),
  _Mode(
    id: 'negative_state_grounding',
    zh: '著陸',
    short: '著陸',
    cap: '濃',
    temp: 'soft',
    intensity: 24,
    tip: '情緒過載 → 一個 ≤10 分鐘行動',
    icon: Icons.anchor,
  ),
  _Mode(
    id: 'serious_engineer',
    zh: '認真工程師',
    short: '工程',
    cap: '關',
    temp: 'cool',
    intensity: 76,
    tip: '程式 / 金錢 / 不可逆 / 正式環境',
    icon: Icons.terminal,
  ),
  _Mode(
    id: 'handoff',
    zh: '交棒',
    short: '交棒',
    cap: '關',
    temp: 'cool',
    intensity: 88,
    tip: '委派編碼 agent + 任務簡報',
    icon: Icons.swap_horiz,
  ),
];

_Mode _findMode(String id) =>
    _kModes.firstWhere((m) => m.id == id, orElse: () => _kModes.first);

// ─── App ──────────────────────────────────────────────────────────────────────

class MikuApp extends StatelessWidget {
  const MikuApp({super.key, required this.client});

  final MikuSessionClient client;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'TempestMiku',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(useMaterial3: true),
      home: MikuHomePage(client: client),
    );
  }
}

// ─── Chat message model ────────────────────────────────────────────────────────

class _Msg {
  final bool isUser;
  final String text; // prefix '\x00mode:' signals a mode-change event
  final String modeId;
  const _Msg({required this.isUser, required this.text, required this.modeId});

  bool get isModeChange => text.startsWith('\x00mode:');
  String get modeChangeId => text.substring(6);
}

// ─── Home page ─────────────────────────────────────────────────────────────────

class MikuHomePage extends StatefulWidget {
  const MikuHomePage({super.key, required this.client});

  final MikuSessionClient client;

  @override
  State<MikuHomePage> createState() => _MikuHomePageState();
}

class _MikuHomePageState extends State<MikuHomePage>
    with SingleTickerProviderStateMixin {
  final _inputCtrl = TextEditingController();
  final _scrollCtrl = ScrollController();
  final List<ApprovalPrompt> _approvals = [];
  final List<String> _nextActions = [];
  final List<_Msg> _history = [];

  Future<void>? _sessionFuture;
  StreamSubscription<MikuEvent>? _sub;
  String? _sessionId;
  String? _lastEventId;
  String _modeId = 'personal_assistant';
  String _status = 'idle';
  String _streamText = '';
  String _projectStatus = '';
  bool _isDark = true;
  bool _modeLocked = false;

  late final AnimationController _dotAnim;

  @override
  void initState() {
    super.initState();
    _dotAnim = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1200),
    )..repeat();
    unawaited(_ensureSession());
  }

  @override
  void dispose() {
    _inputCtrl.dispose();
    _scrollCtrl.dispose();
    _sub?.cancel();
    _dotAnim.dispose();
    super.dispose();
  }

  _Mode get _mode => _findMode(_modeId);
  _Tok get _tok => _isDark ? _Tok.dark : _Tok.light;
  Color get _accent => _modeAccent(_mode.temp, _tok);

  // ── Session ────────────────────────────────────────────────────────────────

  Future<void> _ensureSession() async {
    if (_sessionId != null) return;
    return _sessionFuture ??= _connectSession();
  }

  Future<void> _connectSession() async {
    if (mounted) setState(() => _status = 'connecting');
    try {
      final s = await widget.client.createOrReuseSession();
      if (!mounted) return;
      await _sub?.cancel();
      _sessionId = s.id;
      _lastEventId = s.lastEventId;
      _modeId = s.mode;
      _modeLocked = s.locked;
      _status = 'connected';
      _sub = widget.client
          .events(s.id, lastEventId: _lastEventId)
          .listen(_onEvent, onError: (_) {
        if (mounted) setState(() => _status = 'reconnecting');
      });
      await _loadProject();
      if (mounted) setState(() {});
    } catch (err) {
      _sessionFuture = null;
      if (!mounted) return;
      setState(() => _status = 'offline');
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not connect to tm-server: $err')),
      );
    }
  }

  void _onEvent(MikuEvent e) {
    final eventId = e.id;
    if (eventId != null && eventId.isNotEmpty) {
      _lastEventId = eventId;
      final sessionId = _sessionId;
      if (sessionId != null) {
        widget.client.rememberLastEventId(sessionId, eventId);
      }
    }
    setState(() {
      switch (e.type) {
        case 'connection':
          _status = e.data['status'] as String? ?? _status;
        case 'text':
          _streamText += e.data['delta'] as String? ?? '';
          if (_streamText.isNotEmpty) _status = 'streaming';
        case 'final':
          final text = e.data['text'] as String? ?? '';
          _history.add(_Msg(isUser: false, text: text, modeId: _modeId));
          _streamText = '';
          _status = 'connected';
          _loadProject();
        case 'mode':
          final newId = e.data['mode'] as String? ?? _modeId;
          _modeLocked =
              e.data['lockSource'] != null || e.data['lock_source'] != null;
          if (newId != _modeId) {
            _history.add(
                _Msg(isUser: false, text: '\x00mode:$newId', modeId: newId));
            _modeId = newId;
          }
        case 'approval':
          _approvals.add(ApprovalPrompt(
            approvalId: e.data['approvalId'] as String? ?? '',
            action: e.data['action'] as String? ?? 'Approval requested',
            scope:
                (e.data['scope'] as Map?)?.cast<String, Object?>() ?? const {},
            options: ((e.data['options'] as List?) ?? const [])
                .whereType<Map>()
                .map(
                  (option) => ApprovalOption(
                    optionId: (option['optionId'] as String?) ??
                        (option['option_id'] as String?) ??
                        '',
                    name: (option['name'] as String?) ?? '',
                    kind: (option['kind'] as String?) ?? '',
                  ),
                )
                .where((option) => option.optionId.isNotEmpty)
                .toList(),
            timeoutMs: (e.data['timeoutMs'] as num?)?.toInt() ??
                (e.data['timeout_ms'] as num?)?.toInt(),
          ));
        case 'approval_resolved':
          _approvals.removeWhere((a) => a.approvalId == e.data['approvalId']);
      }
    });
    _scrollToBottom();
  }

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scrollCtrl.hasClients) {
        _scrollCtrl.animateTo(
          _scrollCtrl.position.maxScrollExtent,
          duration: const Duration(milliseconds: 280),
          curve: Curves.easeOut,
        );
      }
    });
  }

  Future<void> _send() async {
    final text = _inputCtrl.text.trim();
    if (text.isEmpty) return;
    await _ensureSession();
    setState(() {
      _history.add(_Msg(isUser: true, text: text, modeId: _modeId));
      _status = 'streaming';
      _streamText = '';
    });
    _inputCtrl.clear();
    await widget.client.sendMessage(_sessionId!, text);
    _scrollToBottom();
  }

  Future<void> _resolve(
    ApprovalPrompt a,
    String decision, {
    String? optionId,
  }) async {
    await widget.client.resolveApproval(
      _sessionId!,
      a.approvalId,
      decision,
      optionId: optionId,
    );
    setState(() => _approvals.remove(a));
  }

  Future<void> _loadProject() async {
    final id = _sessionId;
    if (id == null) return;
    final ov = await widget.client.projectOverview(id);
    if (!mounted) return;
    setState(() {
      _projectStatus = ov.status;
      _nextActions
        ..clear()
        ..addAll(ov.nextActions);
    });
  }

  Future<void> _promoteSession() async {
    await _ensureSession();
    final last = _history.where((m) => !m.isUser && !m.isModeChange).lastOrNull;
    final resources = _extractResources(last?.text ?? '');
    try {
      final p = await widget.client.promoteSession(
        _sessionId!,
        summary: last?.text,
        resources: resources,
      );
      if (!mounted) return;
      setState(() =>
          _projectStatus = '${p.projectUri} · ${p.promotedCount} promoted');
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Promote failed: $e')));
    }
  }

  List<String> _extractResources(String text) {
    return RegExp(r'\b(?:artifact|workspace|linked|project)://[^\s),\]]+')
        .allMatches(text)
        .map((m) => m.group(0)!.replaceAll(RegExp(r'[.。]+$'), ''))
        .toSet()
        .toList();
  }

  Future<void> _openResource(String uri) async {
    await _ensureSession();
    try {
      final preview = await widget.client.previewResource(_sessionId!, uri);
      if (!mounted) return;
      await showModalBottomSheet<void>(
        context: context,
        showDragHandle: true,
        isScrollControlled: true,
        backgroundColor: _tok.surface,
        builder: (_) => _ResourceSheet(preview: preview, tok: _tok),
      );
    } catch (err) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Could not open $uri: $err')));
    }
  }

  // ── Bottom sheets ──────────────────────────────────────────────────────────

  void _showModeSheet() {
    final tok = _tok;
    final accent = _accent;
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: tok.surface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder: (_) => _ModeSheet(
        modes: _kModes,
        currentId: _modeId,
        locked: _modeLocked,
        tok: tok,
        accent: accent,
        onPick: (id) {
          setState(() => _modeId = id);
          Navigator.pop(context);
          if (_modeLocked && _sessionId != null) {
            widget.client.lockMode(_sessionId!, id);
          }
        },
        onLockToggle: () {
          final wasLocked = _modeLocked;
          setState(() => _modeLocked = !_modeLocked);
          Navigator.pop(context);
          if (_sessionId != null) {
            if (!wasLocked) {
              widget.client.lockMode(_sessionId!, _modeId);
            } else {
              widget.client.unlockMode(_sessionId!);
            }
          }
        },
      ),
    );
  }

  void _showApprovalSheet(ApprovalPrompt a) {
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: _tok.surface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder: (_) => _ApprovalSheet(
        approval: a,
        tok: _tok,
        accent: _accent,
        onOption: (option) {
          final isReject = option.kind.startsWith('reject') ||
              option.kind.startsWith('deny');
          _resolve(
            a,
            isReject ? 'deny' : 'approve',
            optionId: option.optionId,
          );
          Navigator.pop(context);
        },
        onApprove: () {
          _resolve(a, 'approve');
          Navigator.pop(context);
        },
        onDeny: () {
          _resolve(a, 'deny');
          Navigator.pop(context);
        },
      ),
    );
  }

  void _showOverflowSheet() {
    final tok = _tok;
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: tok.surface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(20)),
      ),
      builder: (_) => _OverflowSheet(
        tok: tok,
        projectStatus: _projectStatus,
        nextActions: _nextActions,
        onRefresh: () {
          Navigator.pop(context);
          _loadProject();
        },
        onPromote: () {
          Navigator.pop(context);
          _promoteSession();
        },
      ),
    );
  }

  // ── Build ──────────────────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    final tok = _tok;
    final mode = _mode;
    final accent = _accent;

    return AnnotatedRegion<SystemUiOverlayStyle>(
      value: _isDark ? SystemUiOverlayStyle.light : SystemUiOverlayStyle.dark,
      child: Scaffold(
        backgroundColor: tok.bg,
        body: SafeArea(
          bottom: false,
          child: Column(
            children: [
              _buildHeader(tok, mode, accent),
              _buildModeRail(tok, mode, accent),
              const SizedBox(height: 6),
              _buildIntensityMeter(tok, mode, accent),
              const SizedBox(height: 2),
              Expanded(child: _buildThread(tok, mode, accent)),
              _buildComposer(tok, accent),
              SafeArea(
                top: false,
                child: SizedBox(
                  height: 20,
                  child: Center(
                    child: Container(
                      width: 128,
                      height: 5,
                      decoration: BoxDecoration(
                        color: tok.text.withOpacity(0.3),
                        borderRadius: BorderRadius.circular(999),
                      ),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildHeader(_Tok tok, _Mode mode, Color accent) {
    return Container(
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border(
          bottom: BorderSide(color: tok.border.withOpacity(0.6), width: 0.5),
        ),
      ),
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 10),
      child: Row(
        children: [
          Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              color: accent,
              borderRadius: BorderRadius.circular(10),
              boxShadow: [
                BoxShadow(
                  color: accent.withOpacity(0.3),
                  blurRadius: 8,
                  offset: const Offset(0, 3),
                ),
              ],
            ),
            child: Icon(Icons.smart_toy, color: _textOn(accent), size: 19),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  'TempestMiku',
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 15.5,
                    fontWeight: FontWeight.w800,
                    letterSpacing: -0.3,
                    height: 1.1,
                  ),
                ),
                Text(
                  'Miku · ${mode.zh} · voice ${mode.cap}',
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    height: 1.3,
                  ),
                ),
              ],
            ),
          ),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
            decoration: BoxDecoration(
              color: accent.withOpacity(0.12),
              border: Border.all(color: accent.withOpacity(0.45)),
              borderRadius: BorderRadius.circular(999),
            ),
            child: Text(
              _modeLocked ? '${mode.short} · locked' : mode.short,
              style: TextStyle(
                color: accent,
                fontSize: 11,
                fontWeight: FontWeight.w800,
              ),
            ),
          ),
          const SizedBox(width: 8),
          _ConnectionBadge(status: _status, tok: tok),
          const SizedBox(width: 8),
          _TokIconBtn(
            tok: tok,
            icon: Icons.more_horiz,
            onTap: _showOverflowSheet,
          ),
          const SizedBox(width: 6),
          _TokIconBtn(
            tok: tok,
            icon: _isDark ? Icons.wb_sunny_outlined : Icons.nightlight_outlined,
            onTap: () => setState(() => _isDark = !_isDark),
          ),
        ],
      ),
    );
  }

  Widget _buildModeRail(_Tok tok, _Mode mode, Color accent) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 0),
      child: Container(
        padding: const EdgeInsets.all(4),
        decoration: BoxDecoration(
          color: tok.surface,
          border: Border.all(color: tok.border),
          borderRadius: BorderRadius.circular(14),
        ),
        child: Row(
          children: _kModes.map((m) {
            final isActive = m.id == _modeId;
            final mAccent = _modeAccent(m.temp, tok);
            return Expanded(
              child: GestureDetector(
                onTap: () {
                  setState(() => _modeId = m.id);
                  if (_modeLocked && _sessionId != null) {
                    widget.client.lockMode(_sessionId!, m.id);
                  }
                },
                onLongPress: _showModeSheet,
                child: AnimatedContainer(
                  duration: const Duration(milliseconds: 200),
                  padding: const EdgeInsets.symmetric(vertical: 7),
                  decoration: BoxDecoration(
                    color: isActive ? mAccent : Colors.transparent,
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(
                        m.icon,
                        size: 18,
                        color: isActive ? _textOn(mAccent) : tok.muted,
                      ),
                      const SizedBox(height: 3),
                      Text(
                        m.short,
                        style: TextStyle(
                          fontSize: 10,
                          fontWeight: FontWeight.w700,
                          color: isActive ? _textOn(mAccent) : tok.muted,
                          letterSpacing: -0.2,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            );
          }).toList(),
        ),
      ),
    );
  }

  Widget _buildIntensityMeter(_Tok tok, _Mode mode, Color accent) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
      child: Container(
        padding: const EdgeInsets.fromLTRB(13, 9, 13, 10),
        decoration: BoxDecoration(
          color: tok.surface,
          border: Border.all(color: tok.border),
          borderRadius: BorderRadius.circular(13),
        ),
        child: Column(
          children: [
            Row(
              children: [
                Icon(Icons.show_chart, size: 13, color: accent),
                const SizedBox(width: 6),
                Text(
                  '語音強度',
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 11.5,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                const Spacer(),
                Text(
                  mode.tempLabel,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 7),
            LayoutBuilder(
              builder: (_, constraints) {
                final w = constraints.maxWidth;
                final thumbX =
                    (mode.intensity / 100 * w).clamp(8.0, w - 8.0) - 8.0;
                return SizedBox(
                  height: 16,
                  child: Stack(
                    clipBehavior: Clip.none,
                    children: [
                      Positioned.fill(
                        child: Center(
                          child: Container(
                            height: 8,
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(999),
                              gradient: const LinearGradient(
                                colors: [
                                  Color(0xFFB84A30), // hot
                                  Color(0xFFA1736B), // warm/clay
                                  Color(0xFF79837F), // cool
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
                      Positioned(
                        left: thumbX,
                        top: 0,
                        bottom: 0,
                        child: Center(
                          child: Container(
                            width: 16,
                            height: 16,
                            decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              color: tok.bg,
                              border: Border.all(color: accent, width: 3),
                              boxShadow: [
                                BoxShadow(
                                  color: Colors.black.withOpacity(0.2),
                                  blurRadius: 4,
                                ),
                              ],
                            ),
                          ),
                        ),
                      ),
                    ],
                  ),
                );
              },
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                Text(
                  '濃 · 感性',
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 10,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                const Spacer(),
                Text(
                  '中 · 規劃',
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 10,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                const Spacer(),
                Text(
                  '關 · 嚴肅',
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 10,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildThread(_Tok tok, _Mode mode, Color accent) {
    final items = <Widget>[];

    // Connection system line
    items.add(
      _SystemLine(
        tok: tok,
        text: _sessionId != null
            ? '已連線到 lumo${_lastEventId != null ? ' · 從事件 #$_lastEventId 續傳' : ''}'
            : '未連線 · 傳訊息建立連線',
      ),
    );

    // History
    for (final msg in _history) {
      if (msg.isModeChange) {
        final m = _findMode(msg.modeChangeId);
        items.add(_ModeChangeEvent(tok: tok, modeZh: m.zh, modeCap: m.cap));
      } else if (msg.isUser) {
        items.add(
          _UserBubble(tok: tok, text: msg.text, accent: tok.accentSoft),
        );
      } else {
        final m = _findMode(msg.modeId);
        final ma = _modeAccent(m.temp, tok);
        final resources = _extractResources(msg.text);
        items.add(_MikuBubble(
          tok: tok,
          text: msg.text,
          mode: m,
          accent: ma,
          resources: resources,
          onOpenResource: _openResource,
        ));
      }
      items.add(const SizedBox(height: 14));
    }

    // Current stream
    if (_status == 'streaming') {
      if (_streamText.isNotEmpty) {
        final resources = _extractResources(_streamText);
        items.add(_MikuBubble(
          tok: tok,
          text: _streamText,
          mode: mode,
          accent: accent,
          resources: resources,
          onOpenResource: _openResource,
          isStreaming: true,
        ));
      } else {
        items.add(_TypingIndicator(tok: tok, accent: accent, anim: _dotAnim));
      }
      items.add(const SizedBox(height: 14));
    }

    // Pending approvals
    for (final a in _approvals) {
      items.add(
        _ApprovalCard(
          tok: tok,
          approval: a,
          accent: accent,
          onTap: () => _showApprovalSheet(a),
        ),
      );
      items.add(const SizedBox(height: 10));
    }

    return ListView(
      controller: _scrollCtrl,
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 16),
      children: items,
    );
  }

  Widget _buildComposer(_Tok tok, Color accent) {
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 8, 14, 10),
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [tok.bg.withOpacity(0), tok.bg],
          stops: const [0, 0.22],
        ),
      ),
      child: Container(
        decoration: BoxDecoration(
          color: tok.raised,
          border: Border.all(color: tok.border),
          borderRadius: BorderRadius.circular(14),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            Expanded(
              child: TextField(
                controller: _inputCtrl,
                style: TextStyle(color: tok.text, fontSize: 13.5),
                maxLines: null,
                decoration: InputDecoration(
                  hintText: '傳訊息給 Miku…',
                  hintStyle: TextStyle(color: tok.muted, fontSize: 13.5),
                  border: InputBorder.none,
                  contentPadding: const EdgeInsets.fromLTRB(14, 10, 8, 10),
                ),
                onSubmitted: (_) => _send(),
              ),
            ),
            Padding(
              padding: const EdgeInsets.all(5),
              child: GestureDetector(
                onTap: _send,
                child: Container(
                  width: 36,
                  height: 36,
                  decoration: BoxDecoration(
                    color: accent,
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Icon(Icons.send, color: _textOn(accent), size: 17),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ─── Small reusable widgets ────────────────────────────────────────────────────

class _TokIconBtn extends StatelessWidget {
  const _TokIconBtn({
    required this.tok,
    required this.icon,
    required this.onTap,
  });

  final _Tok tok;
  final IconData icon;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        width: 32,
        height: 32,
        decoration: BoxDecoration(
          border: Border.all(color: tok.border),
          borderRadius: BorderRadius.circular(9),
        ),
        child: Icon(icon, color: tok.muted, size: 16),
      ),
    );
  }
}

class _ConnectionBadge extends StatefulWidget {
  const _ConnectionBadge({required this.status, required this.tok});

  final String status;
  final _Tok tok;

  @override
  State<_ConnectionBadge> createState() => _ConnectionBadgeState();
}

class _ConnectionBadgeState extends State<_ConnectionBadge>
    with SingleTickerProviderStateMixin {
  late final AnimationController _pulse;

  @override
  void initState() {
    super.initState();
    _pulse = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2200),
    )..repeat();
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final isLive = widget.status == 'connected' ||
        widget.status == 'streaming' ||
        widget.status == 'complete';
    final dotColor = isLive ? tok.cool : tok.muted;

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          AnimatedBuilder(
            animation: _pulse,
            builder: (_, __) {
              final t = _pulse.value * math.pi * 2;
              final opacity = (math.sin(t) * 0.34 + 0.66).clamp(0.32, 1.0);
              return Opacity(
                opacity: isLive ? opacity : 0.45,
                child: Container(
                  width: 6,
                  height: 6,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: dotColor,
                  ),
                ),
              );
            },
          ),
          const SizedBox(width: 5),
          Text(
            _label(widget.status),
            style: TextStyle(
              color: tok.muted,
              fontSize: 11,
              fontWeight: FontWeight.w700,
            ),
          ),
        ],
      ),
    );
  }

  static String _label(String s) => switch (s) {
        'idle' => '未連線',
        'connecting' => '連線中',
        'connected' => '已連線',
        'streaming' => '回應中',
        'reconnecting' => '重連中',
        'offline' => '離線',
        'complete' => '已連線',
        _ => s,
      };
}

class _SystemLine extends StatelessWidget {
  const _SystemLine({required this.tok, required this.text});

  final _Tok tok;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: Center(
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
          decoration: BoxDecoration(
            color: tok.surface.withOpacity(0.6),
            border: Border.all(color: tok.border.withOpacity(0.7)),
            borderRadius: BorderRadius.circular(999),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 6,
                height: 6,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: tok.cool,
                ),
              ),
              const SizedBox(width: 7),
              Text(
                text,
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _ModeChangeEvent extends StatelessWidget {
  const _ModeChangeEvent({
    required this.tok,
    required this.modeZh,
    required this.modeCap,
  });

  final _Tok tok;
  final String modeZh, modeCap;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Center(
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
          decoration: BoxDecoration(
            color: tok.surface.withOpacity(0.6),
            border: Border.all(color: tok.border.withOpacity(0.7)),
            borderRadius: BorderRadius.circular(999),
          ),
          child: Text(
            '語氣轉為 $modeCap · 切到$modeZh',
            style: TextStyle(
              color: tok.muted,
              fontSize: 11,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
      ),
    );
  }
}

class _UserBubble extends StatelessWidget {
  const _UserBubble({
    required this.tok,
    required this.text,
    required this.accent,
  });

  final _Tok tok;
  final String text;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.centerRight,
      child: Container(
        constraints: const BoxConstraints(maxWidth: 280),
        padding: const EdgeInsets.fromLTRB(13, 10, 13, 10),
        decoration: BoxDecoration(
          color: accent,
          borderRadius: const BorderRadius.only(
            topLeft: Radius.circular(15),
            topRight: Radius.circular(15),
            bottomLeft: Radius.circular(15),
            bottomRight: Radius.circular(5),
          ),
        ),
        child: Text(
          text,
          style: TextStyle(
            color: _textOn(accent),
            fontSize: 14,
            fontWeight: FontWeight.w500,
            height: 1.5,
          ),
        ),
      ),
    );
  }
}

class _MikuBubble extends StatelessWidget {
  const _MikuBubble({
    required this.tok,
    required this.text,
    required this.mode,
    required this.accent,
    required this.resources,
    required this.onOpenResource,
    this.isStreaming = false,
  });

  final _Tok tok;
  final String text;
  final _Mode mode;
  final Color accent;
  final List<String> resources;
  final void Function(String) onOpenResource;
  final bool isStreaming;

  @override
  Widget build(BuildContext context) {
    final iconColor = _textOn(accent);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 30,
          height: 30,
          decoration: BoxDecoration(
            color: accent,
            borderRadius: BorderRadius.circular(9),
          ),
          child: Icon(Icons.smart_toy, color: iconColor, size: 17),
        ),
        const SizedBox(width: 9),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Text(
                    'Miku',
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 14,
                      fontWeight: FontWeight.w800,
                      letterSpacing: -0.3,
                    ),
                  ),
                  const SizedBox(width: 8),
                  Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
                    decoration: BoxDecoration(
                      border: Border.all(color: tok.border),
                      borderRadius: BorderRadius.circular(999),
                    ),
                    child: Text(
                      '${mode.zh} · ${mode.cap}',
                      style: TextStyle(
                        color: tok.muted,
                        fontSize: 10.5,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                  ),
                  if (isStreaming) ...[
                    const SizedBox(width: 6),
                    _PulsingDot(color: accent),
                  ],
                ],
              ),
              const SizedBox(height: 5),
              Text(
                text,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 14,
                  fontWeight: FontWeight.w400,
                  height: 1.62,
                ),
              ),
              if (resources.isNotEmpty) ...[
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: resources
                      .map(
                        (uri) => GestureDetector(
                          onTap: () => onOpenResource(uri),
                          child: Container(
                            padding: const EdgeInsets.symmetric(
                                horizontal: 10, vertical: 6),
                            decoration: BoxDecoration(
                              color: tok.surface,
                              border: Border.all(color: tok.border),
                              borderRadius: BorderRadius.circular(9),
                            ),
                            child: Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                Icon(Icons.insert_drive_file_outlined,
                                    size: 13, color: accent),
                                const SizedBox(width: 6),
                                Text(
                                  uri,
                                  style: TextStyle(
                                    color: accent,
                                    fontSize: 11.5,
                                    fontWeight: FontWeight.w700,
                                    fontFamily: 'monospace',
                                  ),
                                ),
                                const SizedBox(width: 4),
                                Icon(Icons.open_in_new,
                                    size: 11, color: tok.muted),
                              ],
                            ),
                          ),
                        ),
                      )
                      .toList(),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

class _PulsingDot extends StatefulWidget {
  const _PulsingDot({required this.color});

  final Color color;

  @override
  State<_PulsingDot> createState() => _PulsingDotState();
}

class _PulsingDotState extends State<_PulsingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _c;

  @override
  void initState() {
    super.initState();
    _c = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1800),
    )..repeat();
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _c,
      builder: (_, __) {
        final opacity =
            (math.sin(_c.value * math.pi * 2) * 0.34 + 0.66).clamp(0.32, 1.0);
        return Opacity(
          opacity: opacity,
          child: Container(
            width: 6,
            height: 6,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: widget.color,
            ),
          ),
        );
      },
    );
  }
}

class _TypingIndicator extends StatelessWidget {
  const _TypingIndicator({
    required this.tok,
    required this.accent,
    required this.anim,
  });

  final _Tok tok;
  final Color accent;
  final AnimationController anim;

  @override
  Widget build(BuildContext context) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 30,
          height: 30,
          decoration: BoxDecoration(
            color: accent,
            borderRadius: BorderRadius.circular(9),
          ),
          child: Icon(Icons.smart_toy, color: _textOn(accent), size: 17),
        ),
        const SizedBox(width: 9),
        Padding(
          padding: const EdgeInsets.only(top: 7),
          child: AnimatedBuilder(
            animation: anim,
            builder: (_, __) => Row(
              mainAxisSize: MainAxisSize.min,
              children: List.generate(3, (i) {
                final phase = (anim.value - i * 0.18) % 1.0;
                final opacity = (math.sin(phase * math.pi * 2) * 0.4 + 0.6)
                    .clamp(0.25, 1.0);
                final dy =
                    (math.sin(phase * math.pi * 2) * -2.0).clamp(-2.0, 0.0);
                return Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 2),
                  child: Transform.translate(
                    offset: Offset(0, dy),
                    child: Opacity(
                      opacity: opacity,
                      child: Container(
                        width: 6,
                        height: 6,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: tok.muted,
                        ),
                      ),
                    ),
                  ),
                );
              }),
            ),
          ),
        ),
      ],
    );
  }
}

class _ApprovalCard extends StatelessWidget {
  const _ApprovalCard({
    required this.tok,
    required this.approval,
    required this.accent,
    required this.onTap,
  });

  final _Tok tok;
  final ApprovalPrompt approval;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
        decoration: BoxDecoration(
          color: accent.withOpacity(0.09),
          border: Border.all(color: accent.withOpacity(0.45)),
          borderRadius: BorderRadius.circular(13),
        ),
        child: Row(
          children: [
            Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                color: accent,
                borderRadius: BorderRadius.circular(9),
              ),
              child: Icon(
                Icons.warning_amber_rounded,
                color: _textOn(accent),
                size: 18,
              ),
            ),
            const SizedBox(width: 11),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '待核可 · ${approval.action}',
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    '點擊檢視詳情 · 逾時自動拒絕',
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11.5,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
              ),
            ),
            Icon(Icons.chevron_right, color: tok.muted, size: 18),
          ],
        ),
      ),
    );
  }
}

// ─── Bottom sheet widgets ──────────────────────────────────────────────────────

class _ModeSheet extends StatelessWidget {
  const _ModeSheet({
    required this.modes,
    required this.currentId,
    required this.locked,
    required this.tok,
    required this.accent,
    required this.onPick,
    required this.onLockToggle,
  });

  final List<_Mode> modes;
  final String currentId;
  final bool locked;
  final _Tok tok;
  final Color accent;
  final void Function(String) onPick;
  final VoidCallback onLockToggle;

  @override
  Widget build(BuildContext context) {
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(15, 9, 15, 18),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 38,
            height: 5,
            decoration: BoxDecoration(
              color: tok.border,
              borderRadius: BorderRadius.circular(999),
            ),
          ),
          const SizedBox(height: 14),
          Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      '選擇模式',
                      style: TextStyle(
                        color: tok.text,
                        fontSize: 17,
                        fontWeight: FontWeight.w800,
                        letterSpacing: -0.3,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      'Miku 會自動建議，你可隨時鎖定',
                      style: TextStyle(
                        color: tok.muted,
                        fontSize: 12,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ],
                ),
              ),
              _TokIconBtn(
                tok: tok,
                icon: Icons.close,
                onTap: () => Navigator.pop(context),
              ),
            ],
          ),
          const SizedBox(height: 13),
          ...modes.map((m) {
            final isActive = m.id == currentId;
            final mAccent = _modeAccent(m.temp, tok);
            return Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: GestureDetector(
                onTap: () => onPick(m.id),
                child: Container(
                  padding: const EdgeInsets.fromLTRB(12, 11, 12, 11),
                  decoration: BoxDecoration(
                    color: isActive ? mAccent.withOpacity(0.08) : tok.bg,
                    border: Border.all(
                      color: isActive ? mAccent : tok.border,
                    ),
                    borderRadius: BorderRadius.circular(13),
                  ),
                  child: Row(
                    children: [
                      Container(
                        width: 38,
                        height: 38,
                        decoration: BoxDecoration(
                          color: mAccent,
                          borderRadius: BorderRadius.circular(10),
                        ),
                        child: Icon(m.icon, color: _textOn(mAccent), size: 20),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Row(
                              children: [
                                Text(
                                  m.zh,
                                  style: TextStyle(
                                    color: tok.text,
                                    fontSize: 14,
                                    fontWeight: FontWeight.w800,
                                  ),
                                ),
                                const SizedBox(width: 7),
                                Container(
                                  padding:
                                      const EdgeInsets.symmetric(horizontal: 6),
                                  decoration: BoxDecoration(
                                    border: Border.all(color: tok.border),
                                    borderRadius: BorderRadius.circular(999),
                                  ),
                                  child: Text(
                                    m.cap,
                                    style: TextStyle(
                                      color: tok.muted,
                                      fontSize: 9.5,
                                      fontWeight: FontWeight.w800,
                                    ),
                                  ),
                                ),
                              ],
                            ),
                            const SizedBox(height: 2),
                            Text(
                              m.tip,
                              style: TextStyle(
                                color: tok.muted,
                                fontSize: 11.5,
                                fontWeight: FontWeight.w500,
                                height: 1.4,
                              ),
                            ),
                          ],
                        ),
                      ),
                      if (isActive)
                        Container(
                          width: 22,
                          height: 22,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: mAccent,
                          ),
                          child: Icon(Icons.check,
                              color: _textOn(mAccent), size: 14),
                        ),
                    ],
                  ),
                ),
              ),
            );
          }),
          const SizedBox(height: 5),
          GestureDetector(
            onTap: onLockToggle,
            child: Container(
              padding: const EdgeInsets.fromLTRB(13, 12, 13, 12),
              decoration: BoxDecoration(
                color: tok.bg,
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(13),
              ),
              child: Row(
                children: [
                  Icon(
                    locked ? Icons.lock : Icons.lock_open,
                    color: tok.muted,
                    size: 18,
                  ),
                  const SizedBox(width: 11),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '鎖定目前模式',
                          style: TextStyle(
                            color: tok.text,
                            fontSize: 13.5,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                        const SizedBox(height: 1),
                        Text(
                          '鎖定後 Miku 不會自動切換',
                          style: TextStyle(
                            color: tok.muted,
                            fontSize: 11,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                      ],
                    ),
                  ),
                  _Toggle(on: locked, accent: accent, tok: tok),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _Toggle extends StatelessWidget {
  const _Toggle({required this.on, required this.accent, required this.tok});

  final bool on;
  final Color accent;
  final _Tok tok;

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 160),
      width: 46,
      height: 28,
      decoration: BoxDecoration(
        color: on ? accent : tok.border,
        borderRadius: BorderRadius.circular(999),
      ),
      child: AnimatedAlign(
        duration: const Duration(milliseconds: 160),
        alignment: on ? Alignment.centerRight : Alignment.centerLeft,
        child: Padding(
          padding: const EdgeInsets.all(3),
          child: Container(
            width: 22,
            height: 22,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: Colors.white,
              boxShadow: [
                BoxShadow(
                  color: Colors.black.withOpacity(0.15),
                  blurRadius: 3,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _ApprovalSheet extends StatefulWidget {
  const _ApprovalSheet({
    required this.approval,
    required this.tok,
    required this.accent,
    required this.onOption,
    required this.onApprove,
    required this.onDeny,
  });

  final ApprovalPrompt approval;
  final _Tok tok;
  final Color accent;
  final void Function(ApprovalOption option) onOption;
  final VoidCallback onApprove, onDeny;

  @override
  State<_ApprovalSheet> createState() => _ApprovalSheetState();
}

class _ApprovalSheetState extends State<_ApprovalSheet> {
  late final int _initialSecs;
  late int _secs;
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _initialSecs = math.max(
      1,
      ((widget.approval.timeoutMs ?? 60000) / 1000).ceil(),
    );
    _secs = _initialSecs;
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      setState(() => _secs--);
      if (_secs <= 0) {
        _timer?.cancel();
        widget.onDeny();
      }
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final accent = widget.accent;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 9, 16, 18),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 38,
            height: 5,
            decoration: BoxDecoration(
              color: tok.border,
              borderRadius: BorderRadius.circular(999),
            ),
          ),
          const SizedBox(height: 14),
          Row(
            children: [
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  color: accent,
                  borderRadius: BorderRadius.circular(11),
                ),
                child: Icon(
                  Icons.warning_amber_rounded,
                  color: _textOn(accent),
                  size: 21,
                ),
              ),
              const SizedBox(width: 11),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      '需要你核可',
                      style: TextStyle(
                        color: tok.text,
                        fontSize: 17,
                        fontWeight: FontWeight.w800,
                        letterSpacing: -0.3,
                      ),
                    ),
                    const SizedBox(height: 1),
                    Text(
                      'Miku 想執行操作',
                      style: TextStyle(
                        color: tok.muted,
                        fontSize: 12,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ],
                ),
              ),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
                decoration: BoxDecoration(
                  border: Border.all(color: tok.border),
                  borderRadius: BorderRadius.circular(999),
                ),
                child: Text(
                  '關',
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 10,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 13),
          Container(
            width: double.infinity,
            padding: const EdgeInsets.all(13),
            decoration: BoxDecoration(
              color: tok.bg,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(13),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  widget.approval.action,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    fontFamily: 'monospace',
                    height: 1.5,
                  ),
                ),
                if (widget.approval.scope.isNotEmpty) ...[
                  const SizedBox(height: 9),
                  Wrap(
                    spacing: 7,
                    runSpacing: 6,
                    children: widget.approval.scope.entries.map((e) {
                      return Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 9, vertical: 3),
                        decoration: BoxDecoration(
                          color: tok.surface,
                          borderRadius: BorderRadius.circular(999),
                        ),
                        child: Text(
                          '${e.key}: ${e.value}',
                          style: TextStyle(
                            color: tok.muted,
                            fontSize: 11,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      );
                    }).toList(),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(height: 13),
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Text(
                '逾時自動拒絕',
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 11.5,
                  fontWeight: FontWeight.w600,
                ),
              ),
              Text(
                '${_secs}s',
                style: TextStyle(
                  color: tok.text,
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                  fontFamily: 'monospace',
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          ClipRRect(
            borderRadius: BorderRadius.circular(999),
            child: LinearProgressIndicator(
              value: _secs / _initialSecs,
              backgroundColor: tok.border.withOpacity(0.6),
              valueColor: AlwaysStoppedAnimation<Color>(accent),
              minHeight: 5,
            ),
          ),
          const SizedBox(height: 14),
          if (widget.approval.options.isEmpty)
            Row(
              children: [
                Expanded(
                  child: GestureDetector(
                    onTap: widget.onDeny,
                    child: Container(
                      height: 48,
                      decoration: BoxDecoration(
                        color: tok.bg,
                        border: Border.all(color: tok.border),
                        borderRadius: BorderRadius.circular(13),
                      ),
                      child: Center(
                        child: Text(
                          '拒絕',
                          style: TextStyle(
                            color: tok.text,
                            fontSize: 14.5,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  flex: 3,
                  child: GestureDetector(
                    onTap: widget.onApprove,
                    child: Container(
                      height: 48,
                      decoration: BoxDecoration(
                        color: accent,
                        borderRadius: BorderRadius.circular(13),
                      ),
                      child: Center(
                        child: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Icon(Icons.check, color: _textOn(accent), size: 17),
                            const SizedBox(width: 7),
                            Text(
                              '核可並執行',
                              style: TextStyle(
                                color: _textOn(accent),
                                fontSize: 14.5,
                                fontWeight: FontWeight.w800,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
              ],
            )
          else
            Column(
              children: widget.approval.options.map((option) {
                final isReject = option.kind.startsWith('reject') ||
                    option.kind.startsWith('deny');
                final buttonColor = isReject ? tok.bg : accent;
                final textColor = isReject ? tok.text : _textOn(accent);
                return Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: GestureDetector(
                    onTap: () => widget.onOption(option),
                    child: Container(
                      width: double.infinity,
                      height: 46,
                      decoration: BoxDecoration(
                        color: buttonColor,
                        border: Border.all(
                          color: isReject ? tok.border : accent,
                        ),
                        borderRadius: BorderRadius.circular(13),
                      ),
                      child: Center(
                        child: Text(
                          option.name.isEmpty ? option.optionId : option.name,
                          style: TextStyle(
                            color: textColor,
                            fontSize: 14,
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                      ),
                    ),
                  ),
                );
              }).toList(),
            ),
        ],
      ),
    );
  }
}

class _OverflowSheet extends StatelessWidget {
  const _OverflowSheet({
    required this.tok,
    required this.projectStatus,
    required this.nextActions,
    required this.onRefresh,
    required this.onPromote,
  });

  final _Tok tok;
  final String projectStatus;
  final List<String> nextActions;
  final VoidCallback onRefresh, onPromote;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 9, 16, 18),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Center(
            child: Container(
              width: 38,
              height: 5,
              decoration: BoxDecoration(
                color: tok.border,
                borderRadius: BorderRadius.circular(999),
              ),
            ),
          ),
          const SizedBox(height: 14),
          if (projectStatus.isNotEmpty) ...[
            Text(
              '專案狀態',
              style: TextStyle(
                color: tok.text,
                fontSize: 13,
                fontWeight: FontWeight.w700,
              ),
            ),
            const SizedBox(height: 6),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: tok.surface,
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(12),
              ),
              child: Text(
                projectStatus,
                style: TextStyle(
                    color: tok.text, fontSize: 12, fontWeight: FontWeight.w500),
              ),
            ),
            if (nextActions.isNotEmpty) ...[
              const SizedBox(height: 8),
              ...nextActions.map(
                (a) => Padding(
                  padding: const EdgeInsets.only(bottom: 4),
                  child: Row(
                    children: [
                      Icon(Icons.chevron_right, size: 16, color: tok.muted),
                      const SizedBox(width: 4),
                      Expanded(
                        child: Text(
                          a,
                          style: TextStyle(
                            color: tok.text,
                            fontSize: 12.5,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ],
            const SizedBox(height: 12),
          ],
          _ActionRow(
            tok: tok,
            icon: Icons.refresh,
            label: '重新整理專案',
            onTap: onRefresh,
          ),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.upload_file,
            label: '推廣 Session',
            onTap: onPromote,
          ),
        ],
      ),
    );
  }
}

class _ActionRow extends StatelessWidget {
  const _ActionRow({
    required this.tok,
    required this.icon,
    required this.label,
    required this.onTap,
  });

  final _Tok tok;
  final IconData icon;
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
        decoration: BoxDecoration(
          color: tok.bg,
          border: Border.all(color: tok.border),
          borderRadius: BorderRadius.circular(12),
        ),
        child: Row(
          children: [
            Icon(icon, color: tok.muted, size: 18),
            const SizedBox(width: 12),
            Text(
              label,
              style: TextStyle(
                color: tok.text,
                fontSize: 14,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ResourceSheet extends StatelessWidget {
  const _ResourceSheet({required this.preview, required this.tok});

  final ResourcePreview preview;
  final _Tok tok;

  @override
  Widget build(BuildContext context) {
    final title =
        (preview.title?.isNotEmpty == true) ? preview.title! : preview.uri;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                title,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 16,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 5),
              Text(
                '${preview.kind} / ${preview.mime} / ${preview.sizeBytes} bytes',
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 12,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(height: 10),
              SelectableText(
                preview.uri,
                style: TextStyle(color: tok.muted, fontSize: 12),
              ),
              const SizedBox(height: 12),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: tok.raised,
                  border: Border.all(color: tok.border),
                  borderRadius: BorderRadius.circular(8),
                ),
                child: SelectableText(
                  preview.preview.isEmpty ? '(empty preview)' : preview.preview,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12,
                    fontFamily: 'monospace',
                  ),
                ),
              ),
              if (preview.hasMore) ...[
                const SizedBox(height: 8),
                Text(
                  'Preview truncated',
                  style: TextStyle(color: tok.muted, fontSize: 12),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
