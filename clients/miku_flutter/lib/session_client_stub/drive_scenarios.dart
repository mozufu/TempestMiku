part of '../session_client_stub.dart';

extension _ScriptedDriveScenarios on ScriptedMikuClient {
  void _emitDriveWorkspace(
    String sessionId,
    StreamController<MikuEvent> controller,
  ) {
    const path = 'projects/tempestmiku/research/p5-drive-workspace.md';
    const uri = 'drive://projects/tempestmiku/research/p5-drive-workspace.md';
    const movedFrom = 'inbox/raw-research.md';
    const movedFromUri = 'drive://inbox/raw-research.md';
    const proposalId = 'drive-proposal-scripted';
    final item = DriveFeedItem(
      uri: uri,
      path: path,
      title: 'P5 drive research notes',
      docKind: 'note',
      project: 'TempestMiku',
      tags: const ['research', 'p5'],
      contentHash: 'sha256:scripted-drive',
      summary: 'Local drive corpus for P5 research with bounded citations.',
      sizeBytes: 128,
      updatedAt: DateTime.now().toIso8601String(),
    );
    const proposal = DriveOrganizerProposal(
      proposalId: proposalId,
      action: 'move',
      status: 'pending',
      sourcePath: movedFrom,
      sourceUri: movedFromUri,
      proposedPath: path,
      proposedUri: uri,
      confidence: 0.91,
      previewTitle: 'Move drive document',
      previewSubtitle:
          'inbox/raw-research.md -> projects/tempestmiku/research/p5-drive-workspace.md',
      previewSnippet: 'Organizer found the project-scoped research note.',
    );
    _driveFeeds[sessionId] = DriveFeed(
      recent: [item],
      virtualDirs: _defaultDriveVirtualDirs(),
      proposals: [proposal],
      pendingApprovals: const [],
    );

    Map<String, Object?> entryPayload(String action, String title) => {
      'action': action,
      'path': path,
      'uri': uri,
      'title': item.title,
      'docKind': item.docKind,
      'project': item.project,
      'tags': item.tags,
      'mime': 'text/markdown',
      'sizeBytes': item.sizeBytes,
      'contentHash': item.contentHash,
      'preview': {'title': title, 'subtitle': path, 'snippet': item.summary},
      'resourceRefs': [
        {
          'role': 'document',
          'uri': uri,
          'kind': 'drive_document',
          'title': item.title,
          'path': path,
        },
      ],
    };

    final proposalPayload = {
      'proposalId': proposalId,
      'action': 'move',
      'status': 'pending',
      'sourcePath': movedFrom,
      'sourceUri': movedFromUri,
      'proposedPath': path,
      'proposedUri': uri,
      'confidence': 0.91,
      'preview': {
        'title': 'Move drive document',
        'subtitle': '$movedFrom -> $path',
        'snippet': 'Organizer found the project-scoped research note.',
      },
      'resourceRefs': [
        {
          'role': 'source',
          'uri': movedFromUri,
          'kind': 'drive_document',
          'title': 'raw-research.md',
        },
        {
          'role': 'proposed',
          'uri': uri,
          'kind': 'drive_document',
          'title': 'p5-drive-workspace.md',
        },
      ],
    };

    controller.add(
      MikuEvent(
        type: 'project_linked',
        id: _eventId(),
        data: const {
          'action': 'link',
          'alias': 'tempestmiku',
          'linkedUri': 'linked://tempestmiku',
          'mode': 'rw',
          'project': 'TempestMiku',
          'memoryScope': 'project:tempestmiku',
          'preview': {
            'title': 'Linked project folder',
            'subtitle': 'TempestMiku -> linked://tempestmiku',
            'snippet': '/Users/brian/TempestMiku',
          },
          'resourceRefs': [
            {
              'role': 'linked',
              'uri': 'linked://tempestmiku',
              'kind': 'linked_folder',
              'title': 'TempestMiku',
            },
          ],
        },
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_put',
        id: _eventId(),
        data: entryPayload('put', 'Filed drive document'),
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_tagged',
        id: _eventId(),
        data: entryPayload('tag', 'Tagged drive document'),
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_moved',
        id: _eventId(),
        data: {
          ...entryPayload('move', 'Moved drive document'),
          'fromPath': movedFrom,
          'fromUri': movedFromUri,
          'toPath': path,
          'toUri': uri,
        },
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_organizer_started',
        id: _eventId(),
        data: const {
          'apply': false,
        },
      ),
    );
    controller.add(
      MikuEvent(
        type: 'drive_organizer_completed',
        id: _eventId(),
        data: {
          'apply': false,
          'runId': 'scripted-run',
          'proposalCount': 1,
          'proposals': [proposalPayload],
          'resourceRefs': proposalPayload['resourceRefs'],
        },
      ),
    );
  }
}

List<DriveVirtualDir> _defaultDriveVirtualDirs() {
  return const [
    DriveVirtualDir(
      uri: 'drive://recent',
      name: 'recent',
      kind: 'virtual_dir',
      title: 'Recent documents',
    ),
    DriveVirtualDir(
      uri: 'drive://by-project',
      name: 'by-project',
      kind: 'virtual_dir',
      title: 'Documents by project',
    ),
    DriveVirtualDir(
      uri: 'drive://by-type',
      name: 'by-type',
      kind: 'virtual_dir',
      title: 'Documents by type',
    ),
    DriveVirtualDir(
      uri: 'drive://by-tag',
      name: 'by-tag',
      kind: 'virtual_dir',
      title: 'Documents by tag',
    ),
    DriveVirtualDir(
      uri: 'drive://by-date',
      name: 'by-date',
      kind: 'virtual_dir',
      title: 'Documents by date',
    ),
  ];
}
