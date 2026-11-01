import Foundation
import XCTest

@testable import tauri_plugin_mobile_native

final class SpeechModelTests: XCTestCase {
  private func manifest(_ model: SpeechModelID) throws -> ModelManifest {
    try JSONDecoder().decode(ModelManifest.self, from: PinnedSpeechManifest.data(for: model))
  }

  func testPinnedModelsHaveCompleteDistinctAssets() throws {
    let english = try manifest(.parakeetV2)
    let multilingual = try manifest(.parakeetV3)
    try english.validate(for: .parakeetV2)
    try multilingual.validate(for: .parakeetV3)
    XCTAssertNotEqual(english.revision, multilingual.revision)
    XCTAssertEqual(english.totalBytes, 464_413_247)
    XCTAssertTrue(english.files.contains { $0.path == "JointDecision.mlmodelc/model.mil" })
    XCTAssertTrue(multilingual.files.contains { $0.path == "JointDecisionv3.mlmodelc/model.mil" })
    XCTAssertEqual(multilingual.revision, "7dd20fe6b1797d35f5e3307e8b1732d9a178edfe")
    XCTAssertEqual(SpeechModelID.parakeetV2.version.blankId, 1024)
    XCTAssertEqual(SpeechModelID.parakeetV3.version.blankId, 8192)
    XCTAssertThrowsError(try english.validate(for: .parakeetV3))
  }

  func testManifestRejectsUnpinnedRevisionAndDuplicateFiles() throws {
    let pinned = try manifest(.parakeetV2)
    let unpinned = ModelManifest(
      repository: pinned.repository, revision: "main", files: pinned.files)
    XCTAssertThrowsError(try unpinned.validate(for: .parakeetV2))
    var duplicate = pinned.files
    duplicate[0] = duplicate[1]
    XCTAssertThrowsError(
      try ModelManifest(repository: pinned.repository, revision: pinned.revision, files: duplicate)
        .validate(for: .parakeetV2))
  }

  func testManifestRejectsUnsafePathsAndInvalidChecksums() throws {
    let pinned = try manifest(.parakeetV2)
    for invalid in [
      ModelFile(path: "../Encoder.mlmodelc/model.mil", size: 1, sha256: pinned.files[0].sha256),
      ModelFile(path: pinned.files[0].path, size: 1, sha256: "unverified"),
    ] {
      var files = pinned.files
      files[0] = invalid
      XCTAssertThrowsError(
        try ModelManifest(repository: pinned.repository, revision: pinned.revision, files: files)
          .validate(for: .parakeetV2))
    }
  }

  func testIdleStatusUsesSelectedPublicIDAndSize() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: root) }
    let speech = NativeSpeech(baseDirectory: root)
    for model in SpeechModelID.allCases {
      let status = try await speech.status(modelId: model.rawValue)
      XCTAssertEqual(status.modelId, model.rawValue)
      XCTAssertEqual(status.phase, "idle")
      XCTAssertFalse(status.ready)
      XCTAssertFalse(status.downloading)
      XCTAssertEqual(status.downloadedBytes, 0)
      XCTAssertEqual(status.totalBytes, try manifest(model).totalBytes)
    }
  }

  func testDeletePreservesOtherModelRevisionFolder() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: root) }
    let englishRoot = root.appendingPathComponent(try manifest(.parakeetV2).revision)
    let multilingualRoot = root.appendingPathComponent(try manifest(.parakeetV3).revision)
    try FileManager.default.createDirectory(at: englishRoot, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(at: multilingualRoot, withIntermediateDirectories: true)
    let speech = NativeSpeech(baseDirectory: root)
    let status = try await speech.delete(modelId: "parakeet-v2")
    XCTAssertEqual(status.phase, "idle")
    XCTAssertFalse(FileManager.default.fileExists(atPath: englishRoot.path))
    XCTAssertTrue(FileManager.default.fileExists(atPath: multilingualRoot.path))
  }

  func testEverySpeechOperationRejectsUnknownModelBeforeIO() async throws {
    let speech = NativeSpeech(baseDirectory: URL(fileURLWithPath: "/unavailable-model-test"))
    do {
      _ = try await speech.status(modelId: "other")
      XCTFail("Status accepted unknown model")
    } catch { XCTAssertTrue(error.localizedDescription.contains("Unknown speech model")) }
    do {
      _ = try await speech.download(modelId: "other")
      XCTFail("Download accepted unknown model")
    } catch { XCTAssertTrue(error.localizedDescription.contains("Unknown speech model")) }
    do {
      _ = try await speech.delete(modelId: "other")
      XCTFail("Delete accepted unknown model")
    } catch { XCTAssertTrue(error.localizedDescription.contains("Unknown speech model")) }
    do {
      _ = try await speech.transcribe(
        TranscriptionArgs(
          modelId: "other", path: "/missing.wav", offsetSeconds: 0, durationSeconds: 1))
      XCTFail("Transcription accepted unknown model")
    } catch { XCTAssertTrue(error.localizedDescription.contains("Unknown speech model")) }
  }

  func testArgumentsRequireExplicitModelChoice() throws {
    XCTAssertThrowsError(try JSONDecoder().decode(SpeechModelArgs.self, from: Data("{}".utf8)))
    XCTAssertThrowsError(
      try JSONDecoder().decode(
        TranscriptionArgs.self,
        from: Data(#"{"path":"audio.wav","offsetSeconds":0,"durationSeconds":1}"#.utf8)))
  }

  func testProgressTracksValidatedFilesAndDiscardsFailedCurrentFile() {
    let progress = DownloadProgress()
    progress.beginFile(size: 100)
    progress.finishFile()
    progress.beginFile(size: 200)
    let task = URLSession.shared.downloadTask(with: URL(string: "https://example.com/model")!)
    progress.urlSession(
      .shared, downloadTask: task, didWriteData: 50, totalBytesWritten: 50,
      totalBytesExpectedToWrite: 200)
    XCTAssertEqual(progress.bytes, 150)
    progress.urlSession(
      .shared, downloadTask: task, didWriteData: 500, totalBytesWritten: 550,
      totalBytesExpectedToWrite: 200)
    XCTAssertEqual(progress.bytes, 300)
    progress.discardCurrentFile()
    XCTAssertEqual(progress.bytes, 100)
    progress.reset()
    XCTAssertEqual(progress.bytes, 0)
  }
}
