part of 'main.dart';

// ─── Runtime mode view model ──────────────────────────────────────────────────

class _Mode {
  final String id, label, short, temp, tip, voiceCap, capabilityClass;
  final List<String> activeSkills;
  final double intensity; // 0-100
  final IconData icon;

  const _Mode({
    required this.id,
    required this.label,
    required this.short,
    required this.temp,
    required this.tip,
    required this.voiceCap,
    required this.capabilityClass,
    required this.activeSkills,
    required this.intensity,
    required this.icon,
  });

  factory _Mode.fromProfile(ModeProfile profile) {
    final temp = _modeTemp(profile);
    return _Mode(
      id: profile.id,
      label: profile.label.isEmpty ? profile.id : profile.label,
      short: _shortModeLabel(
        profile.label.isEmpty ? profile.id : profile.label,
      ),
      temp: temp,
      tip:
          profile.description.isEmpty
              ? '${profile.capabilityClass} · ${profile.voiceCap}'
              : profile.description,
      voiceCap: profile.voiceCap,
      capabilityClass: profile.capabilityClass,
      activeSkills: profile.activeSkills,
      intensity: _modeIntensity(profile),
      icon: _modeIcon(profile),
    );
  }

  factory _Mode.fallback(String id) {
    final profile = ModeProfile(
      id: id.isEmpty ? 'runtime_mode' : id,
      label: id.isEmpty ? 'Runtime Mode' : id.replaceAll('_', ' '),
      voiceCap: 'medium',
      defaultScope: 'global',
      capabilityClass: 'conversation',
      activeSkills: const [],
      capabilities: const [],
      description: 'Runtime mode profile unavailable.',
    );
    return _Mode.fromProfile(profile);
  }
}

_Mode _findMode(String id, List<_Mode> modes) =>
    modes.firstWhere((m) => m.id == id, orElse: () => _Mode.fallback(id));

String _shortModeLabel(String label) {
  final words = label.trim().split(RegExp(r'\s+')).where((w) => w.isNotEmpty);
  if (words.isEmpty) return 'Mode';
  final first = words.first;
  return first.length <= 8 ? first : '${first.substring(0, 8)}...';
}

String _modeTemp(ModeProfile profile) {
  if (profile.hasCapability('backend.coding') ||
      profile.hasCapability('agents.spawn')) {
    return 'cool';
  }
  if (profile.voiceCap == 'high') return 'hot';
  if (profile.id.contains('ground') || profile.id.contains('negative')) {
    return 'soft';
  }
  return 'warm';
}

double _modeIntensity(ModeProfile profile) {
  if (profile.hasCapability('agents.spawn')) return 88;
  if (profile.hasCapability('backend.coding')) return 76;
  if (profile.voiceCap == 'high') return 34;
  if (profile.voiceCap == 'off') return 14;
  return 46;
}

IconData _modeIcon(ModeProfile profile) {
  if (profile.hasCapability('agents.spawn')) return Icons.swap_horiz;
  if (profile.hasCapability('backend.coding')) return Icons.terminal;
  if (profile.id.contains('ground') || profile.id.contains('negative')) {
    return Icons.anchor;
  }
  if (profile.voiceCap == 'high') return Icons.bolt;
  return Icons.chat_bubble_outline;
}
