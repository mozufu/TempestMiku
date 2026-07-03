part of 'main.dart';

// ─── Mode definitions ─────────────────────────────────────────────────────────

class _Mode {
  final String id, zh, short, temp, tip;
  final double intensity; // 0–100
  final IconData icon;
  const _Mode({
    required this.id,
    required this.zh,
    required this.short,
    required this.temp,
    required this.tip,
    required this.intensity,
    required this.icon,
  });
}

const _kModes = <_Mode>[
  _Mode(
    id: 'personal_assistant',
    zh: '個人助理',
    short: '助理',
    temp: 'warm',
    intensity: 46,
    tip: '規劃 · 提醒 · 寫作 · 開放迴圈',
    icon: Icons.chat_bubble_outline,
  ),
  _Mode(
    id: 'ambiguity_grill',
    zh: '燒烤我',
    short: '燒烤',
    temp: 'hot',
    intensity: 8,
    tip: '模糊 / 矛盾 → 3–7 個尖銳提問',
    icon: Icons.bolt,
  ),
  _Mode(
    id: 'negative_state_grounding',
    zh: '著陸',
    short: '著陸',
    temp: 'soft',
    intensity: 24,
    tip: '情緒過載 → 一個 ≤10 分鐘行動',
    icon: Icons.anchor,
  ),
  _Mode(
    id: 'serious_engineer',
    zh: '認真工程師',
    short: '工程',
    temp: 'cool',
    intensity: 76,
    tip: '程式 / 金錢 / 不可逆 / 正式環境',
    icon: Icons.terminal,
  ),
  _Mode(
    id: 'handoff',
    zh: '交棒',
    short: '交棒',
    temp: 'cool',
    intensity: 88,
    tip: '委派編碼 agent + 任務簡報',
    icon: Icons.swap_horiz,
  ),
];

_Mode _findMode(String id) =>
    _kModes.firstWhere((m) => m.id == id, orElse: () => _kModes.first);
