import Foundation
import QuickLook
import UIKit

enum NativeFiles {
  static func fileURL(path: String) throws -> URL {
    let url = URL(fileURLWithPath: path)
    let values = try url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
    guard values.isRegularFile == true, values.isSymbolicLink != true,
      FileManager.default.isReadableFile(atPath: path)
    else { throw NativeFailure(message: "This attachment is unavailable on this device.") }
    return url
  }

  @MainActor
  static func preview(path: String, from controller: UIViewController?) throws {
    let url = try fileURL(path: path)
    guard QLPreviewController.canPreview(url as NSURL) else {
      throw NativeFailure(message: "This attachment type cannot be previewed on iPhone.")
    }
    guard let controller, controller.viewIfLoaded?.window != nil else {
      throw NativeFailure(message: "Opening an attachment needs a visible app window.")
    }
    guard controller.presentedViewController == nil else {
      throw NativeFailure(message: "Close the current dialog before opening an attachment.")
    }
    controller.present(FilePreviewController(url: url), animated: true)
  }
}

final class FilePreviewController: QLPreviewController, QLPreviewControllerDataSource {
  private let url: URL

  init(url: URL) {
    self.url = url
    super.init(nibName: nil, bundle: nil)
    dataSource = self
  }

  required init?(coder: NSCoder) { nil }

  func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }

  func previewController(_ controller: QLPreviewController, previewItemAt index: Int)
    -> QLPreviewItem
  {
    url as NSURL
  }
}
