part of 'main.dart';

String _formatDriveUpdatedAt(String value, _UiCopy copy) {
  final parsed = DateTime.tryParse(value);
  if (parsed == null) return copy.recent;
  final local = parsed.toLocal();
  final month = local.month.toString().padLeft(2, '0');
  final day = local.day.toString().padLeft(2, '0');
  final hour = local.hour.toString().padLeft(2, '0');
  final minute = local.minute.toString().padLeft(2, '0');
  return '$month/$day $hour:$minute';
}

String _formatDriveBytes(int size) {
  if (size < 1024) return '$size B';
  final kb = size / 1024;
  if (kb < 1024) return '${kb.toStringAsFixed(kb < 10 ? 1 : 0)} KB';
  final mb = kb / 1024;
  return '${mb.toStringAsFixed(mb < 10 ? 1 : 0)} MB';
}

class _ResourceSheet extends StatelessWidget {
  const _ResourceSheet({
    required this.preview,
    required this.tok,
    required this.copy,
  });

  final ResourcePreview preview;
  final _Tok tok;
  final _UiCopy copy;

  @override
  Widget build(BuildContext context) {
    final title =
        (preview.title?.isNotEmpty == true) ? preview.title! : preview.uri;
    final body =
        preview.content.trim().isNotEmpty ? preview.content : preview.preview;
    final isPreviewOnly = preview.content.trim().isEmpty;
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
                  body.isEmpty ? copy.emptyPreview : body,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12,
                    fontFamily: 'monospace',
                  ),
                ),
              ),
              if (isPreviewOnly && preview.hasMore) ...[
                const SizedBox(height: 8),
                Text(
                  copy.previewTruncated,
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
