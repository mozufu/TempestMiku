part of '../main.dart';

class _DriveFeedDocRow extends StatelessWidget {
  const _DriveFeedDocRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.item,
    required this.onOpen,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final DriveFeedItem item;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: copy.openResource(item.uri),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-feed:${item.uri}'),
          onTap: onOpen,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: Container(
            padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
            decoration: BoxDecoration(
              color: tok.bg,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(12),
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Container(
                  width: 34,
                  height: 34,
                  decoration: BoxDecoration(
                    color: accent.withValues(alpha: 0.14),
                    border: Border.all(color: accent.withValues(alpha: 0.42)),
                    borderRadius: BorderRadius.circular(9),
                  ),
                  child: Icon(
                    Icons.insert_drive_file_outlined,
                    color: accent,
                    size: 17,
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        item.displayTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 13,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                      const SizedBox(height: 3),
                      Text(
                        item.displayPreview,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 11.5,
                          fontWeight: FontWeight.w600,
                          height: 1.35,
                        ),
                      ),
                      const SizedBox(height: 7),
                      Wrap(
                        spacing: 6,
                        runSpacing: 6,
                        children: [
                          if (item.docKind?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.description_outlined,
                              text: item.docKind!,
                            ),
                          if (item.project?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.folder_outlined,
                              text: item.project!,
                            ),
                          if (item.tags.isNotEmpty)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.label_outline,
                              text: copy.driveTags(item.tags.length),
                            ),
                          if (item.sizeBytes != null)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.data_object,
                              text: _formatDriveBytes(item.sizeBytes!),
                            ),
                          if (item.updatedAt?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.schedule,
                              text: _formatDriveUpdatedAt(
                                item.updatedAt!,
                                copy,
                              ),
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 6),
                Icon(Icons.chevron_right, color: tok.muted, size: 17),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _DriveVirtualDirChip extends StatelessWidget {
  const _DriveVirtualDirChip({
    required this.tok,
    required this.dir,
    required this.onOpen,
  });

  final _Tok tok;
  final DriveVirtualDir dir;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final label = dir.title.isEmpty ? dir.name : dir.title;
    return Semantics(
      button: true,
      label: label,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-dir:${dir.uri}'),
          onTap: onOpen,
          borderRadius: BorderRadius.circular(999),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 6),
            decoration: BoxDecoration(
              color: tok.bg,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(999),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(Icons.folder_special_outlined, color: tok.muted, size: 13),
                const SizedBox(width: 6),
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 190),
                  child: Text(
                    label,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 11.3,
                      fontWeight: FontWeight.w800,
                    ),
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
