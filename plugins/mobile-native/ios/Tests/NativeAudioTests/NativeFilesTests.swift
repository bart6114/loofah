import Foundation
import XCTest

@testable import tauri_plugin_mobile_native

final class NativeFilesTests: XCTestCase {
  func testPreviewAcceptsLocalRegularFilesAndRejectsMissingFilesAndDirectories() throws {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let file = directory.appendingPathComponent("note.txt")
    try Data("A real attachment".utf8).write(to: file)
    XCTAssertEqual(try NativeFiles.fileURL(path: file.path), file)
    XCTAssertThrowsError(try NativeFiles.fileURL(path: directory.path))
    XCTAssertThrowsError(
      try NativeFiles.fileURL(path: directory.appendingPathComponent("missing.txt").path))
  }

  func testPreviewRejectsSymlinkFiles() throws {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let file = directory.appendingPathComponent("private.txt")
    try Data("private".utf8).write(to: file)
    let link = directory.appendingPathComponent("link.txt")
    try FileManager.default.createSymbolicLink(at: link, withDestinationURL: file)
    XCTAssertThrowsError(try NativeFiles.fileURL(path: link.path))
  }
  @MainActor
  func testPreviewControllerReleasesItsOwnWeakDataSource() {
    for _ in 0..<20 {
      weak var released: FilePreviewController?
      autoreleasepool {
        let preview = FilePreviewController(url: URL(fileURLWithPath: "/tmp/attachment.txt"))
        XCTAssertTrue(preview.dataSource === preview)
        released = preview
      }
      XCTAssertNil(released, "Preview controller retained its own data source")
    }
  }
}
