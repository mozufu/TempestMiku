import 'dart:async';

import 'package:flutter/material.dart';

import 'pairing_scanner_service.dart';
import 'session_models.dart';

export 'pairing_scanner_service.dart';

/// A review-first camera route for one-time TempestMiku pairing payloads.
///
/// A successful scan only returns the exact QR payload to its caller. It does
/// not exchange the code or alter the selected server; the caller must still
/// show the normal authority review before pairing.
class PairingScannerPage extends StatefulWidget {
  const PairingScannerPage({super.key, this.service});

  @visibleForTesting
  final PairingScannerService? service;

  @override
  State<PairingScannerPage> createState() => _PairingScannerPageState();
}

class _PairingScannerPageState extends State<PairingScannerPage>
    with WidgetsBindingObserver {
  late final PairingScannerService _service;
  late final bool _ownsService;
  late final StreamSubscription<PairingScannerEvent> _events;
  Future<void> _cameraOperation = Future<void>.value();
  PairingScannerProblem? _problem;
  bool _starting = true;
  bool _pausedForLifecycle = false;
  bool _rejectedPayload = false;
  bool _handled = false;
  bool _disposed = false;

  @override
  void initState() {
    super.initState();
    _ownsService = widget.service == null;
    _service = widget.service ?? createPairingScannerService();
    _events = _service.events.listen(
      _onScannerEvent,
      onError: (_, _) => _showProblem(PairingScannerProblem.cameraError),
    );
    WidgetsBinding.instance.addObserver(this);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _startCamera();
    });
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (_handled || _disposed) return;
    switch (state) {
      case AppLifecycleState.resumed:
        if (!_pausedForLifecycle) return;
        _pausedForLifecycle = false;
        _startCamera();
      case AppLifecycleState.inactive:
      case AppLifecycleState.hidden:
      case AppLifecycleState.paused:
      case AppLifecycleState.detached:
        if (_pausedForLifecycle) return;
        _pausedForLifecycle = true;
        _queueCameraOperation(_service.stop);
    }
  }

  void _onScannerEvent(PairingScannerEvent event) {
    if (!mounted || _handled) return;
    switch (event.type) {
      case PairingScannerEventType.ready:
        if (_pausedForLifecycle) {
          return;
        }
        setState(() {
          _starting = false;
          _problem = null;
        });
      case PairingScannerEventType.payload:
        final rawValue = event.rawValue;
        if (rawValue != null) _handlePayload(rawValue);
      case PairingScannerEventType.problem:
        _showProblem(event.problem ?? PairingScannerProblem.cameraError);
    }
  }

  void _handlePayload(String rawValue) {
    if (!_isExactVersionOnePairingPayload(rawValue)) {
      if (!_rejectedPayload) {
        setState(() => _rejectedPayload = true);
      }
      return;
    }

    _handled = true;
    _queueCameraOperation(_service.stop);
    Navigator.of(context).pop(rawValue);
  }

  bool _isExactVersionOnePairingPayload(String rawValue) {
    if (rawValue.isEmpty || rawValue != rawValue.trim()) return false;
    try {
      final uri = Uri.parse(rawValue);
      final values = uri.queryParametersAll;
      if (uri.scheme != 'tempestmiku' ||
          uri.host != 'pair' ||
          uri.path.isNotEmpty ||
          uri.hasFragment ||
          values.length != 3 ||
          values.keys.toSet().difference(const {
            'v',
            'server',
            'code',
          }).isNotEmpty ||
          values.values.any((items) => items.length != 1) ||
          values['v']?.single != '1') {
        return false;
      }
      pairingTargetFromLink(rawValue);
      return true;
    } on FormatException {
      return false;
    }
  }

  void _showProblem(PairingScannerProblem problem) {
    if (!mounted || _handled) return;
    setState(() {
      _starting = false;
      _problem = problem;
    });
  }

  void _startCamera() {
    if (_handled || _disposed) return;
    setState(() {
      _starting = true;
      _problem = null;
      _rejectedPayload = false;
    });
    _queueCameraOperation(_service.start);
  }

  void _queueCameraOperation(Future<void> Function() operation) {
    _cameraOperation = _cameraOperation.then((_) async {
      if (_disposed) return;
      try {
        await operation();
      } on Exception {
        _showProblem(PairingScannerProblem.cameraError);
      }
    });
  }

  void _cancel() {
    if (_handled) return;
    _handled = true;
    _queueCameraOperation(_service.stop);
    Navigator.of(context).pop();
  }

  @override
  void dispose() {
    _disposed = true;
    WidgetsBinding.instance.removeObserver(this);
    unawaited(_events.cancel());
    final stop = _cameraOperation
        .then((_) => _service.stop())
        .catchError((_) {});
    if (_ownsService) {
      unawaited(stop.then((_) => _service.dispose()));
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final problem = _problem;
    return PopScope<String?>(
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) {
          _handled = true;
        }
      },
      child: Scaffold(
        backgroundColor: Colors.black,
        appBar: AppBar(
          backgroundColor: Colors.black,
          foregroundColor: Colors.white,
          leading: IconButton(
            key: const Key('pairing-scanner-cancel'),
            tooltip: '取消掃描',
            onPressed: _cancel,
            icon: const Icon(Icons.close_rounded),
          ),
          title: const Text('掃描安全配對 QR'),
        ),
        body: Stack(
          fit: StackFit.expand,
          children: [
            Semantics(
              label: '相機 QR 掃描預覽',
              image: true,
              child: _service.buildPreview(),
            ),
            IgnorePointer(
              child: Center(
                child: LayoutBuilder(
                  builder: (context, constraints) {
                    final side = constraints.biggest.shortestSide.clamp(
                      180.0,
                      260.0,
                    );
                    return Semantics(
                      label: '將配對 QR 對準掃描框',
                      child: Container(
                        width: side,
                        height: side,
                        decoration: BoxDecoration(
                          border: Border.all(
                            color: const Color(0xff5fd0c5),
                            width: 3,
                          ),
                          borderRadius: BorderRadius.circular(18),
                        ),
                      ),
                    );
                  },
                ),
              ),
            ),
            if (problem != null)
              _PairingScannerProblemCard(
                problem: problem,
                onRetry:
                    problem == PairingScannerProblem.unsupported
                        ? null
                        : _startCamera,
                onClose: _cancel,
              ),
            if (problem == null)
              Positioned(
                left: 20,
                right: 20,
                bottom: 24,
                child: SafeArea(
                  top: false,
                  child: Semantics(
                    liveRegion: true,
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        color: Colors.black.withValues(alpha: 0.78),
                        borderRadius: BorderRadius.circular(16),
                      ),
                      child: Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 16,
                          vertical: 12,
                        ),
                        child: Text(
                          _statusText(),
                          textAlign: TextAlign.center,
                          style: const TextStyle(
                            color: Colors.white,
                            fontSize: 15,
                            height: 1.4,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  String _statusText() {
    if (_starting) return '正在開啟相機…';
    if (_rejectedPayload) {
      return '這不是有效的 TempestMiku v1 配對 QR；尚未變更任何配對。';
    }
    return '只掃描你自己的 tm-server 配對頁所顯示的一次性 QR。';
  }
}

class _PairingScannerProblemCard extends StatelessWidget {
  const _PairingScannerProblemCard({
    required this.problem,
    required this.onRetry,
    required this.onClose,
  });

  final PairingScannerProblem problem;
  final VoidCallback? onRetry;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final message = switch (problem) {
      PairingScannerProblem.permissionDenied =>
        '請在系統設定允許 TempestMiku 使用相機，回到這裡後再試一次。',
      PairingScannerProblem.unsupported => '你仍可返回設定頁，貼上一次性 TempestMiku 配對連結。',
      PairingScannerProblem.cameraError => '請確認沒有其他程式占用相機，然後再試一次。',
    };

    return ColoredBox(
      color: Colors.black.withValues(alpha: 0.72),
      child: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: Semantics(
            liveRegion: true,
            child: Container(
              constraints: const BoxConstraints(maxWidth: 420),
              padding: const EdgeInsets.all(22),
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.surface,
                borderRadius: BorderRadius.circular(22),
              ),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    problem == PairingScannerProblem.permissionDenied
                        ? Icons.no_photography_outlined
                        : Icons.camera_alt_outlined,
                    size: 36,
                    color: Theme.of(context).colorScheme.primary,
                  ),
                  const SizedBox(height: 14),
                  Text(
                    message,
                    textAlign: TextAlign.center,
                    style: Theme.of(context).textTheme.bodyLarge,
                  ),
                  const SizedBox(height: 20),
                  Wrap(
                    alignment: WrapAlignment.center,
                    spacing: 10,
                    runSpacing: 10,
                    children: [
                      OutlinedButton(
                        key: const Key('pairing-scanner-close'),
                        style: OutlinedButton.styleFrom(
                          minimumSize: const Size(44, 44),
                        ),
                        onPressed: onClose,
                        child: const Text('返回'),
                      ),
                      if (onRetry != null)
                        FilledButton.icon(
                          key: const Key('pairing-scanner-retry'),
                          style: FilledButton.styleFrom(
                            minimumSize: const Size(44, 44),
                          ),
                          onPressed: onRetry,
                          icon: const Icon(Icons.refresh_rounded),
                          label: const Text('重試'),
                        ),
                    ],
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
