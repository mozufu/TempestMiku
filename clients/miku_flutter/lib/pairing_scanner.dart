part of 'main.dart';

class PairingAuthorityDetails extends StatelessWidget {
  const PairingAuthorityDetails({
    super.key,
    required this.target,
    required this.deviceName,
  });

  final MikuPairingTarget target;
  final String deviceName;

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SelectableText('Origin: ${target.origin}'),
        Text('Scheme: ${target.scheme}'),
        Text('Host: ${target.host}'),
        Text('Effective port: ${target.effectivePort}'),
        Text('Device name: $deviceName'),
        const SizedBox(height: 16),
        const Text(
          'This device will receive owner access to sessions, files, and approvals. '
          'Continue only if you recognize this server.',
        ),
      ],
    );
  }
}

class PairingScannerPage extends StatefulWidget {
  const PairingScannerPage({
    super.key,
    @visibleForTesting this.controller,
    @visibleForTesting this.preview,
  });

  final MobileScannerController? controller;
  final Widget? preview;

  @override
  State<PairingScannerPage> createState() => _PairingScannerPageState();
}

class _PairingScannerPageState extends State<PairingScannerPage>
    with WidgetsBindingObserver {
  late final MobileScannerController _controller;
  late final bool _ownsController;
  Future<void> _cameraLifecycleOperation = Future.value();
  bool _handled = false;
  bool _pausedForLifecycle = false;
  bool _disposed = false;

  @override
  void initState() {
    super.initState();
    _ownsController = widget.controller == null;
    _controller =
        widget.controller ??
        MobileScannerController(formats: const [BarcodeFormat.qrCode]);
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (_handled || !_controller.value.hasCameraPermission) return;

    switch (state) {
      case AppLifecycleState.resumed:
        if (!_pausedForLifecycle) return;
        _pausedForLifecycle = false;
        _queueCameraLifecycleOperation(_controller.start);
      case AppLifecycleState.inactive:
      case AppLifecycleState.hidden:
      case AppLifecycleState.paused:
      case AppLifecycleState.detached:
        if (_pausedForLifecycle) return;
        _pausedForLifecycle = true;
        _queueCameraLifecycleOperation(_controller.stop);
    }
  }

  void _queueCameraLifecycleOperation(Future<void> Function() operation) {
    _cameraLifecycleOperation = _cameraLifecycleOperation.then((_) async {
      if (_disposed) return;
      try {
        await operation();
      } on Exception catch (error) {
        debugPrint('Pairing scanner lifecycle transition failed: $error');
      }
    });
  }

  @override
  void dispose() {
    _disposed = true;
    WidgetsBinding.instance.removeObserver(this);
    if (_ownsController) {
      unawaited(_controller.dispose());
    }
    super.dispose();
  }

  void _onDetect(BarcodeCapture capture) {
    if (_handled) return;
    for (final barcode in capture.barcodes) {
      final value = barcode.rawValue?.trim();
      if (value == null || !value.startsWith('tempestmiku://pair?')) {
        continue;
      }
      _handled = true;
      _controller.stop();
      Navigator.of(context).pop(value);
      return;
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        title: const Text('Scan secure pairing QR'),
        backgroundColor: Colors.black,
        foregroundColor: Colors.white,
      ),
      body: Stack(
        fit: StackFit.expand,
        children: [
          widget.preview ??
              MobileScanner(
                controller: _controller,
                onDetect: _onDetect,
                useAppLifecycleState: false,
              ),
          IgnorePointer(
            child: Center(
              child: Container(
                width: 260,
                height: 260,
                decoration: BoxDecoration(
                  border: Border.all(color: const Color(0xFF5FD0C5), width: 3),
                  borderRadius: BorderRadius.circular(18),
                ),
              ),
            ),
          ),
          const Positioned(
            left: 24,
            right: 24,
            bottom: 36,
            child: Text(
              'Scan only a QR shown by your own tm-server pairing page.',
              textAlign: TextAlign.center,
              style: TextStyle(color: Colors.white, fontSize: 16),
            ),
          ),
        ],
      ),
    );
  }
}
