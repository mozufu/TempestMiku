part of 'main.dart';

enum _UiLanguage { en, zh }

class _UiCopy {
  const _UiCopy(this.language);

  final _UiLanguage language;

  bool get isZh => language == _UiLanguage.zh;

  String pick(String en, String zh) => isZh ? zh : en;

  String get code => isZh ? '中' : 'EN';
  String get nextCode => isZh ? 'EN' : '中';
  String get languageTooltip => pick('Switch language', '切換語言');
  String get languageSemantic => pick(
        'Current language: English. Switch to Traditional Chinese',
        '目前語言：繁體中文。切換為英文',
      );

  String get sessions => pick('Sessions', '歷史');
  String get openSessions => pick('Open sessions', '開啟歷史');
  String get more => pick('More', '更多');
  String get openMore => pick('Open more actions', '開啟更多操作');
  String get close => pick('Close', '關閉');
  String get refresh => pick('Refresh', '重新整理');
  String get newSession => pick('New session', '新 session');
  String get createNewSession => pick('Create new session', '建立新 session');

  String get switchMode => pick('Switch mode', '切換模式');
  String get modeLocked => pick('Mode locked', '模式已鎖定');
  String currentMode(_Mode mode) => pick(
        'Current mode: ${mode.label}',
        '目前模式：${mode.label}',
      );
  String currentModeLocked(_Mode mode) => pick(
        'Current mode locked: ${mode.label}',
        '目前鎖定模式：${mode.label}',
      );
  String modeChipLabel(_Mode mode, bool locked) =>
      locked ? pick('${mode.short} locked', '${mode.short} 鎖定') : mode.short;

  String statusLabel(String status) => switch (status) {
        'idle' => pick('Offline', '未連線'),
        'connecting' => pick('Connecting', '連線中'),
        'connected' => pick('Online', '已連線'),
        'streaming' => pick('Responding', '回應中'),
        'reconnecting' => pick('Reconnecting', '重連中'),
        'offline' => pick('Offline', '離線'),
        'complete' => pick('Online', '已連線'),
        _ => status,
      };
  String connectionStatus(String label) =>
      pick('Connection status: $label', '連線狀態：$label');
  String systemConnected(String? lastEventId) => pick(
        'Connected to lumo${lastEventId != null ? ' · event #$lastEventId' : ''}',
        '已連線到 lumo${lastEventId != null ? ' · 事件 #$lastEventId' : ''}',
      );
  String get systemOffline =>
      pick('Offline · send to connect', '未連線 · 送出訊息以連線');

  String get emptyTitle => pick('Miku is here', 'Miku 在這裡');
  String get messageField => pick('Message Miku', '傳訊息給 Miku');
  String get messageHint => pick('Message Miku...', '傳訊息給 Miku...');
  String get send => pick('Send', '送出');
  String get typeMessage => pick('Type a message', '輸入訊息');
  String get sendMessage => pick('Send message', '送出訊息');

  String round(int index) => pick('Round $index', '回合 $index');
  String get openAgentActivity => pick('Open agent activity', '開啟 agent 活動');
  String agentsSummary(int running, int stopped) => pick(
        'Agents · $running running / $stopped stopped',
        'Agents · $running 執行中 / $stopped 已停止',
      );
  String events(int count) => pick('$count events', '$count 個事件');
  String get runtimeStatus => pick('Runtime status', 'Runtime 狀態');
  String stateLabel(_ActivityState state) => switch (state) {
        _ActivityState.running => pick('running', '執行中'),
        _ActivityState.done => pick('stopped', '已停止'),
        _ActivityState.failed => pick('failed', '失敗'),
        _ActivityState.info => pick('updated', '已更新'),
      };
  String openResource(String uri) => pick('Open resource $uri', '開啟資源 $uri');
  String openActivityResource(String uri) =>
      pick('Open activity resource $uri', '開啟活動資源 $uri');

  String get modeSheetTitle => pick('Mode / Lock', '模式 / 鎖定');
  String get modeSheetHelper => pick(
        'Pick a mode manually; lock keeps it until you unlock.',
        '手動選擇模式；鎖定後會保持目前模式。',
      );
  String get closeModeSheet => pick('Close mode sheet', '關閉模式選單');
  String lockMode(_Mode mode) => pick('Lock ${mode.short}', '鎖定 ${mode.short}');
  String unlockMode(_Mode mode) =>
      pick('Unlock ${mode.short}', '解除鎖定 ${mode.short}');
  String get lockModeHelper =>
      pick('Keep this mode until unlocked', '保持目前模式直到解除鎖定');
  String get unlockModeHelper =>
      pick('Restore Miku auto-routing', '恢復 Miku 自動路由');
  String selectMode(_Mode mode) =>
      pick('Select mode ${mode.label}', '選擇模式 ${mode.label}');

  String agentsSheetTitle(int roundIndex) =>
      pick('Agents · Round $roundIndex', 'Agents · 回合 $roundIndex');
  String agentCount(int agents, int events) =>
      pick('$agents agents · $events events', '$agents 個 agent · $events 個事件');
  String get closeActivitySheet => pick('Close activity sheet', '關閉活動面板');
  String get status => pick('Status', '狀態');
  String get promptActivity => pick('Prompt / Activity', '提示 / 活動');

  String get historyHelper =>
      pick('Switch history or start fresh', '切換歷史對話或建立新 session');
  String get refreshSessions => pick('Refresh sessions', '重新整理 sessions');
  String get closeSessions => pick('Close sessions', '關閉歷史 session');
  String openSession(String title) =>
      pick('Open session $title', '開啟 session $title');
  String messages(int count) => pick('$count messages', '$count 則訊息');
  String get recent => pick('recent', '最近');
  String get loadingSessions => pick('Loading sessions...', '載入 sessions...');
  String historyLoadFailed(Object error) =>
      pick('Could not load: $error', '讀取失敗：$error');
  String get noSessions => pick('No sessions yet', '還沒有歷史 session');

  String get approvalNeeded => pick('Approval needed', '需要你核可');
  String get approvalHelper =>
      pick('Miku wants to run an action', 'Miku 想執行操作');
  String get autoDeny => pick('Auto-deny on timeout', '逾時自動拒絕');
  String get deny => pick('Deny', '拒絕');
  String get approveOnce => pick('Approve once', '核可一次');
  String pendingApproval(String action) => pick(
        'Pending approval · $action',
        '待核可 · $action',
      );
  String pendingApprovalSemantics(String action) =>
      pick('Pending approval: $action', '待核可：$action');
  String get tapForDetails =>
      pick('Tap for details · timeout auto-denies', '點擊檢視 · 逾時自動拒絕');

  String get memoryProposal => pick('Memory proposal', '記憶提案');
  String get pending => pick('pending', '待核可');
  String get syncing => pick('syncing', '同步中');
  String get saveMemory => pick('Save memory', '存入記憶');
  String get waitingForApproval => pick('Waiting for approval', '等待核可');
  String get waiting => pick('Waiting', '等待中');
  String scopeChip(String scope) => pick('scope $scope', '範圍 $scope');
  String provenanceChip(String provenance) =>
      pick('provenance $provenance', '來源 $provenance');

  String get projectStatus => pick('Project status', '專案狀態');
  String get lightMode => pick('Light mode', '淺色模式');
  String get darkMode => pick('Dark mode', '深色模式');
  String get refreshProject => pick('Refresh project', '重新整理專案');
  String get promoteSession => pick('Promote Session', '推廣 Session');
  String get modeSettings => pick('Mode settings', '模式與鎖定');
  String get previewTruncated => pick('Preview truncated', '預覽已截斷');
  String get emptyPreview => pick('(empty preview)', '（空白預覽）');
}
