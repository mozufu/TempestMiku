part of '../main.dart';

class _SettingsSectionLabel extends StatelessWidget {
  const _SettingsSectionLabel({required this.tok, required this.label});

  final _Tok tok;
  final String label;

  @override
  Widget build(BuildContext context) {
    return Text(
      label,
      style: Theme.of(
        context,
      ).textTheme.labelLarge?.copyWith(color: tok.muted, letterSpacing: 0.4),
    );
  }
}

class _ActionRow extends StatelessWidget {
  const _ActionRow({
    required this.tok,
    required this.icon,
    required this.label,
    required this.onTap,
    this.supportingText,
    this.semanticLabel,
    this.trailing,
    this.foregroundColor,
  });

  final _Tok tok;
  final IconData icon;
  final String label;
  final VoidCallback onTap;
  final String? supportingText;
  final String? semanticLabel;
  final Widget? trailing;
  final Color? foregroundColor;

  @override
  Widget build(BuildContext context) {
    final color = foregroundColor ?? tok.text;
    final textTheme = Theme.of(context).textTheme;
    return Semantics(
      button: true,
      excludeSemantics: true,
      label: semanticLabel ?? label,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(16),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 56),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
              decoration: BoxDecoration(
                color: tok.raised,
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(16),
              ),
              child: Row(
                children: [
                  Icon(icon, color: foregroundColor ?? tok.muted, size: 22),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          label,
                          style: textTheme.titleSmall?.copyWith(
                            color: color,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                        if (supportingText != null) ...[
                          const SizedBox(height: 2),
                          Text(
                            supportingText!,
                            style: textTheme.bodySmall?.copyWith(
                              color:
                                  foregroundColor?.withValues(alpha: 0.88) ??
                                  tok.muted,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  if (trailing != null) ...[
                    const SizedBox(width: 10),
                    trailing!,
                  ],
                  const SizedBox(width: 4),
                  Icon(
                    Icons.chevron_right_rounded,
                    color: foregroundColor ?? tok.muted,
                    size: 22,
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
