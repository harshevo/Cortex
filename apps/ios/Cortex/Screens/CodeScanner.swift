#if os(iOS)
import AVFoundation
import SwiftUI

/// The camera, looking for a QR code. Reports each new code once.
struct CodeScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        let controller = ScannerController()

        controller.found = found

        return controller
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {}
}

final class ScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var found: ((String) -> Void)?

    private let session = AVCaptureSession()
    private var preview: AVCaptureVideoPreviewLayer?
    private var last: String?

    override func viewDidLoad() {
        super.viewDidLoad()

        view.backgroundColor = .black

        AVCaptureDevice.requestAccess(for: .video) { [weak self] allowed in
            guard allowed else { return }

            DispatchQueue.main.async {
                self?.start()
            }
        }
    }

    private func start() {
        guard
            let camera = AVCaptureDevice.default(for: .video),
            let input = try? AVCaptureDeviceInput(device: camera),
            session.canAddInput(input)
        else { return }

        session.addInput(input)

        let output = AVCaptureMetadataOutput()

        guard session.canAddOutput(output) else { return }

        session.addOutput(output)

        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let preview = AVCaptureVideoPreviewLayer(session: session)

        preview.videoGravity = .resizeAspectFill
        preview.frame = view.bounds

        view.layer.addSublayer(preview)

        self.preview = preview

        // Starting the camera blocks, so not on the main thread.
        DispatchQueue.global(qos: .userInitiated).async { [session] in
            session.startRunning()
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()

        preview?.frame = view.bounds
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)

        session.stopRunning()
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput objects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard
            let code = (objects.first as? AVMetadataMachineReadableCodeObject)?.stringValue,
            code != last
        else { return }

        last = code

        found?(code)
    }
}
#endif
