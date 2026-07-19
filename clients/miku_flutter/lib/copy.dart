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
  String get driveFeed => pick('Drive', 'Drive');
  String get openDriveFeed => pick('Open drive feed', '開啟 Drive 動態');
  String get more => pick('More', '更多');
  String get openMore => pick('Open more actions', '開啟更多操作');
  String get close => pick('Close', '關閉');
  String get cancel => pick('Cancel', '取消');
  String get save => pick('Save', '儲存');
  String get refresh => pick('Refresh', '重新整理');
  String get newSession => pick('New session', '新 session');
  String get createNewSession => pick('Create new session', '建立新 session');

  String get switchMode => pick('Switch mode', '切換模式');
  String get modeLocked => pick('Mode locked', '模式已鎖定');
  String currentMode(_Mode mode) =>
      pick('Current mode: ${mode.label}', '目前模式：${mode.label}');
  String currentModeLocked(_Mode mode) =>
      pick('Current mode locked: ${mode.label}', '目前鎖定模式：${mode.label}');
  String modeChipLabel(_Mode mode, bool locked) =>
      locked ? pick('${mode.short} locked', '${mode.short} 鎖定') : mode.short;

  String statusLabel(String status) => switch (status) {
    'idle' => pick('Offline', '未連線'),
    'connecting' => pick('Connecting', '連線中'),
    'connected' => pick('Online', '已連線'),
    'streaming' => pick('Responding', '回應中'),
    'reconnecting' => pick('Reconnecting', '重連中'),
    'offline' => pick('Offline', '離線'),
    'ended' => pick('Ended', '已結束'),
    'complete' => pick('Online', '已連線'),
    _ => status,
  };
  String connectionStatus(String label) =>
      pick('Connection status: $label', '連線狀態：$label');

  String get emptyTitle => pick('Miku is here', 'Miku 在這裡');
  String get messageField => pick('Message Miku', '傳訊息給 Miku');
  String get messageHint => pick('Message Miku...', '傳訊息給 Miku...');
  String get sessionEndedHint =>
      pick('This session has ended', '此 session 已結束');
  String get sessionEnded => pick('Session ended', 'Session 已結束');
  String get send => pick('Send', '送出');
  String get typeMessage => pick('Type a message', '輸入訊息');
  String get sendMessage => pick('Send message', '送出訊息');
  String sendFailed(Object error) =>
      pick('Message not sent: $error', '訊息未送出：$error');
  String get shareWithMiku => pick('Share with Miku', '分享給 Miku');
  String get askMikuAboutThis => pick('Ask Miku about this', '問問 Miku 這段內容');
  String get quickCapture => pick('Quick capture', '快速記錄');
  String get voiceCapture => pick('Voice capture', '語音記錄');
  String get shareReviewHelper =>
      pick('Review and edit before anything is sent.', '送出前先確認並編輯內容。');
  String get shareTruncated => pick(
    'This share exceeded the safe import limit and was shortened.',
    '分享內容超過安全匯入上限，已截短。',
  );
  String get quickCaptureTruncated => pick(
    'This draft exceeded the safe capture limit and was shortened.',
    '草稿超過安全記錄上限，已截短。',
  );
  String get sharedContent => pick('Shared content', '分享內容');
  String get selectedText => pick('Selected text', '選取的文字');
  String get captureDraft => pick('Capture draft', '記錄草稿');
  String get transcriptDraft => pick('Transcript draft', '轉錄草稿');
  String get sharedFromAndroid => pick('Shared from Android', '來自 Android 分享');
  String get selectedFromAndroid =>
      pick('Selected in another Android app', '從其他 Android 應用程式選取');
  String get quickCaptureFromAndroid =>
      pick('Opened from an Android shortcut or tile', '由 Android 捷徑或快速設定開啟');
  String get voiceCapturedOnDevice =>
      pick('Transcribed locally on this device', '已在此裝置本機轉錄');
  String get voiceCapturedSelfHosted => pick(
    'Transcribed by your fixed self-hosted home service',
    '已由固定的家用自架服務轉錄',
  );
  String get recordVoice => pick('Record voice', '錄製語音');
  String get stopVoice => pick('Stop and transcribe', '停止並轉錄');
  String get cancelVoice => pick('Cancel voice capture', '取消語音記錄');
  String get voiceEngineUnavailable =>
      pick('The selected voice engine is unavailable', '目前選取的語音引擎無法使用');
  String get voiceModelUnavailable =>
      pick('On-device voice model is not installed', '尚未安裝裝置端語音模型');
  String get voicePermissionDenied =>
      pick('Microphone permission was not granted.', '未授予麥克風權限。');
  String voiceCaptureFailed(Object error) =>
      pick('Voice capture failed: $error', '語音記錄失敗：$error');
  String get voiceTranscriptEmpty => pick(
    'The selected engine returned an empty transcript.',
    '目前選取的引擎沒有產生轉錄文字。',
  );
  String voiceCaptureQualityWarning(
    VoiceCaptureQualityIssue issue,
  ) => switch (issue) {
    VoiceCaptureQualityIssue.tooShort => pick(
      'The recording was very short. Check the draft or record it again.',
      '錄音時間很短，請檢查草稿或重新錄製。',
    ),
    VoiceCaptureQualityIssue.tooQuiet => pick(
      'The recording was nearly silent. Move closer to the microphone and try again.',
      '收音幾乎沒有聲音，請靠近麥克風後重試。',
    ),
    VoiceCaptureQualityIssue.clipped => pick(
      'The recording was clipped. Move slightly away from the microphone and try again.',
      '收音有明顯爆音，請稍微遠離麥克風後重試。',
    ),
  };
  String get voiceCaptureDiagnosticsTitle =>
      pick('Recording diagnostics', '錄音診斷');
  String voiceCaptureDiagnosticsPrivacy(
    VoiceTranscriptProvenance provenance,
  ) => switch (provenance) {
    VoiceTranscriptProvenance.local => pick(
      'Diagnostics contain aggregate measurements only. Audio uses app-private temporary storage during capture, is deleted during cleanup, and is never sent.',
      '診斷僅包含彙總測量值。音訊只在錄音期間使用應用程式私有暫存，清理時會刪除，且絕不傳送。',
    ),
    VoiceTranscriptProvenance.selfHosted => pick(
      'Diagnostics contain aggregate measurements only. This recording was sent through your paired TempestMiku server to the fixed home ASR service, then erased from the app during cleanup.',
      '診斷僅包含彙總測量值。這段錄音已經由配對的 TempestMiku Server 傳到固定的家用 ASR 服務，並在清理時從應用程式抹除。',
    ),
  };
  String voiceCaptureId(String captureId) =>
      pick('Capture ID $captureId', '錄音 ID $captureId');
  String get voiceBuildFingerprintTitle =>
      pick('App build fingerprint', '應用程式版本指紋');
  String get voiceBuildFingerprintUnavailable => pick(
    'Build fingerprint unavailable. Transcription was not blocked.',
    '無法讀取版本指紋；轉錄仍照常完成。',
  );
  String voiceBuildFingerprintSummary(VoiceAppBuildFingerprint fingerprint) =>
      '${fingerprint.applicationId}\n'
      '${fingerprint.versionName}+${fingerprint.versionCode} · '
      '${fingerprint.buildType}\n'
      'APK SHA-256 ${fingerprint.apkSha256}';
  String voiceCaptureDiagnosticsSummary(VoiceCaptureDiagnostics diagnostics) {
    final durationSeconds =
        diagnostics.duration.inMilliseconds / Duration.millisecondsPerSecond;
    final activePercent = diagnostics.activeFrameFraction * 100;
    final clippedPercent = diagnostics.clippedFraction * 100;
    final nearZeroPercent = diagnostics.nearZeroFraction * 100;
    return pick(
      'Duration ${durationSeconds.toStringAsFixed(2)} s · '
          'RMS ${diagnostics.rmsDbfs.toStringAsFixed(1)} dBFS · '
          'peak ${diagnostics.peakDbfs.toStringAsFixed(1)} dBFS · '
          'active ${activePercent.toStringAsFixed(1)}% · '
          'clipped ${clippedPercent.toStringAsFixed(2)}% · '
          'near-zero ${nearZeroPercent.toStringAsFixed(1)}% · '
          'lead/trail ${diagnostics.leadingSilence.inMilliseconds}/'
          '${diagnostics.trailingSilence.inMilliseconds} ms',
      '時長 ${durationSeconds.toStringAsFixed(2)} 秒 · '
          'RMS ${diagnostics.rmsDbfs.toStringAsFixed(1)} dBFS · '
          '峰值 ${diagnostics.peakDbfs.toStringAsFixed(1)} dBFS · '
          '有效語音 ${activePercent.toStringAsFixed(1)}% · '
          '削波 ${clippedPercent.toStringAsFixed(2)}% · '
          '近零 ${nearZeroPercent.toStringAsFixed(1)}% · '
          '前／後靜音 ${diagnostics.leadingSilence.inMilliseconds}/'
          '${diagnostics.trailingSilence.inMilliseconds} 毫秒',
    );
  }

  String get sendTo => pick('Send to', '傳送到');
  String get currentChat => pick('Current chat', '目前對話');
  String get newChat => pick('New chat', '新對話');
  String get sendToMiku => pick('Send to Miku', '傳給 Miku');
  String shareSendFailed(Object error) =>
      pick('Could not send shared content: $error', '無法送出分享內容：$error');

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
  String get thinking => pick('Thinking', '思考過程');
  String get thinkingTrace => pick(
    'Private chain-of-thought from the model. Tap to expand or collapse.',
    '模型的私密思考過程。點擊展開或收合。',
  );
  String get reasoningHidden =>
      pick('Reasoning hidden by the provider', '提供者未回傳推理過程');

  String get historyHelper =>
      pick('Switch history or start fresh', '切換歷史對話或建立新 session');
  String get refreshSessions => pick('Refresh sessions', '重新整理 sessions');
  String get closeSessions => pick('Close sessions', '關閉歷史 session');
  String openSession(String title) =>
      pick('Open session $title', '開啟 session $title');
  String messages(int count) => pick('$count messages', '$count 則訊息');
  String get recent => pick('recent', '最近');
  String get loadingSessions => pick('Loading sessions...', '載入 sessions...');
  String get noSessions => pick('No sessions yet', '還沒有歷史 session');

  String get approvalNeeded => pick('Approval needed', '需要你核可');
  String get approvalHelper =>
      pick('Miku wants to run an action', 'Miku 想執行操作');
  String get autoDeny => pick('Auto-deny on timeout', '逾時自動拒絕');
  String get deny => pick('Deny', '拒絕');
  String get approveOnce => pick('Approve once', '核可一次');
  String pendingApproval(String action) =>
      pick('Pending approval · $action', '待核可 · $action');
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
  String get refreshDrive => pick('Refresh Drive', '重新整理 Drive');
  String get closeDriveFeed => pick('Close drive feed', '關閉 Drive 動態');
  String get driveFeedHelper => pick(
    'Recent docs, virtual folders, and organizer proposals',
    '最近文件、虛擬資料夾與整理提案',
  );
  String get loadingDriveFeed =>
      pick('Loading drive feed...', '載入 Drive 動態...');
  String get noDriveFeed => pick('No drive documents yet', '還沒有 Drive 文件');
  String driveFeedLoadFailed(Object error) =>
      pick('Could not load Drive: $error', 'Drive 讀取失敗：$error');
  String get recentDocuments => pick('Recent documents', '最近文件');
  String get virtualDirs => pick('Virtual folders', '虛擬資料夾');
  String get organizerProposals => pick('Organizer proposals', '整理提案');
  String get pendingDriveApprovals =>
      pick('Pending drive approvals', '待核可 Drive 操作');
  String driveDocs(int count) => pick('$count docs', '$count 份文件');
  String driveProposals(int count) => pick('$count proposals', '$count 個提案');
  String driveConfidence(double value) => pick(
    '${(value * 100).round()}% confidence',
    '${(value * 100).round()}% 信心',
  );
  String driveTags(int count) => pick('$count tags', '$count 個標籤');
  String get modeSettings => pick('Mode settings', '模式與鎖定');
  String get serverTarget => pick('Server target', 'Server 目標');
  String get serverUrl => pick('Server URL', 'Server URL');
  String pairedToServer(String url) => pick('Paired to $url', '已配對到 $url');
  String pairingLinkFailed(Object error) =>
      pick('Could not use pairing link: $error', '配對連結無法使用：$error');
  String serverTargetFailed(Object error) =>
      pick('Could not update server target: $error', '更新 Server 目標失敗：$error');
  String get previewTruncated => pick('Preview truncated', '預覽已截斷');
  String get emptyPreview => pick('(empty preview)', '（空白預覽）');
}
