import Foundation
import Tauri
import UIKit
import UniformTypeIdentifiers

private final class ICloudVaultPresenter: NSObject, NSFilePresenter {
  let presentedItemURL: URL?
  let presentedItemOperationQueue: OperationQueue = {
    let queue = OperationQueue()
    queue.name = "Loofah iCloud vault changes"
    queue.maxConcurrentOperationCount = 1
    queue.qualityOfService = .utility
    return queue
  }()

  init(url: URL) {
    presentedItemURL = url
    super.init()
  }
  func presentedItemDidChange() { nativeEvent("icloud_changed") }
  func presentedSubitemDidChange(at url: URL) { nativeEvent("icloud_changed") }
  func presentedSubitemDidAppear(at url: URL) { nativeEvent("icloud_changed") }
  func presentedSubitem(at oldURL: URL, didMoveTo newURL: URL) { nativeEvent("icloud_changed") }
  func presentedItemDidMove(to newURL: URL) {
    nativeEvent(
      "icloud_moved", message: "The iCloud vault moved. Choose its folder again to reconnect.")
  }
  func accommodatePresentedSubitemDeletion(
    at url: URL, completionHandler: @escaping (Error?) -> Void
  ) {
    nativeEvent("icloud_changed")
    completionHandler(nil)
  }
  func accommodatePresentedItemDeletion(completionHandler: @escaping (Error?) -> Void) {
    nativeEvent(
      "icloud_moved",
      message: "The selected iCloud vault is unavailable. Choose its folder again to reconnect.")
    completionHandler(nil)
  }
}

final class ICloudVaultAccess: NSObject, UIDocumentPickerDelegate {
  static let shared = ICloudVaultAccess()
  private let bookmarkKey = "loofah.icloud-vault.bookmark.v1"
  private var scopedURL: URL?
  private var pending: Invoke?
  private var presenter: ICloudVaultPresenter?

  private func installPresenter(_ url: URL) {
    if let presenter { NSFileCoordinator.removeFilePresenter(presenter) }
    let next = ICloudVaultPresenter(url: url)
    presenter = next
    NSFileCoordinator.addFilePresenter(next)
  }

  private func validateICloud(_ url: URL) throws {
    let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isUbiquitousItemKey])
    guard values.isDirectory == true, values.isUbiquitousItem == true else {
      throw NativeFailure(
        message:
          "Choose a folder in iCloud Drive. Other Files providers are not supported for vault sync."
      )
    }
  }

  func select(_ invoke: Invoke) {
    guard pending == nil else {
      invoke.reject("A folder picker is already open.")
      return
    }
    guard
      let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first(
        where: { $0.activationState == .foregroundActive }),
      var controller = scene.windows.first(where: { $0.isKeyWindow })?.rootViewController
    else {
      invoke.reject("Open Loofah before choosing an iCloud vault.")
      return
    }
    while let presented = controller.presentedViewController { controller = presented }
    let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
    picker.allowsMultipleSelection = false
    picker.delegate = self
    pending = invoke
    controller.present(picker, animated: true)
  }

  func restore(_ invoke: Invoke) {
    do {
      if let scopedURL {
        invoke.resolve(["path": scopedURL.path])
        return
      }
      guard let data = UserDefaults.standard.data(forKey: bookmarkKey) else {
        invoke.resolve(["path": NSNull()])
        return
      }
      var stale = false
      let url = try URL(
        resolvingBookmarkData: data, options: [.withoutUI], relativeTo: nil,
        bookmarkDataIsStale: &stale)
      guard url.startAccessingSecurityScopedResource() else {
        throw NativeFailure(
          message: "iCloud folder access expired. Choose the vault again in Files.")
      }
      do {
        try validateICloud(url)
        if stale {
          let renewed = try url.bookmarkData(
            options: [.minimalBookmark], includingResourceValuesForKeys: nil, relativeTo: nil)
          UserDefaults.standard.set(renewed, forKey: bookmarkKey)
        }
      } catch {
        url.stopAccessingSecurityScopedResource()
        throw error
      }
      scopedURL = url
      installPresenter(url)
      invoke.resolve(["path": url.path])
    } catch { invoke.reject(error.localizedDescription) }
  }

  func disconnect(_ invoke: Invoke) {
    if let presenter { NSFileCoordinator.removeFilePresenter(presenter) }
    presenter = nil
    scopedURL?.stopAccessingSecurityScopedResource()
    scopedURL = nil
    UserDefaults.standard.removeObject(forKey: bookmarkKey)
    invoke.resolve()
  }

  func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL])
  {
    guard let invoke = pending else { return }
    pending = nil
    guard let url = urls.first else {
      invoke.reject("No folder was selected.")
      return
    }
    do {
      guard url.startAccessingSecurityScopedResource() else {
        throw NativeFailure(message: "Files did not grant access to this folder.")
      }
      do {
        try validateICloud(url)
        let data = try url.bookmarkData(
          options: [.minimalBookmark], includingResourceValuesForKeys: nil, relativeTo: nil)
        scopedURL?.stopAccessingSecurityScopedResource()
        scopedURL = url
        installPresenter(url)
        UserDefaults.standard.set(data, forKey: bookmarkKey)
        // Access stays active for Rust NSFileCoordinator reconciliation;
        // releasing it here invalidates access as soon as the picker closes.
        invoke.resolve(["path": url.path])
      } catch {
        url.stopAccessingSecurityScopedResource()
        throw error
      }
    } catch { invoke.reject(error.localizedDescription) }
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    pending?.reject("Folder selection cancelled.")
    pending = nil
  }

  func download(_ invoke: Invoke, relativePath: String) {
    guard let scopedURL else {
      invoke.reject("Choose an iCloud vault first.")
      return
    }
    let components = relativePath.split(separator: "/", omittingEmptySubsequences: false)
    guard !components.isEmpty,
      components.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." && !$0.contains("\\") })
    else {
      invoke.reject("Invalid vault path.")
      return
    }
    let url = scopedURL.appendingPathComponent(relativePath)
    let resolved = url.resolvingSymlinksInPath().standardizedFileURL.path
    let root = scopedURL.resolvingSymlinksInPath().standardizedFileURL.path + "/"
    guard resolved.hasPrefix(root) else {
      invoke.reject("Vault path escapes the selected folder.")
      return
    }
    DispatchQueue.global(qos: .utility).async {
      do {
        try FileManager.default.startDownloadingUbiquitousItem(at: url)
        invoke.resolve()
      } catch { invoke.reject(error.localizedDescription) }
    }
  }
}
