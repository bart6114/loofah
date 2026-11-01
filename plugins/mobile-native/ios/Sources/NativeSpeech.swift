import AVFoundation
import CoreML
import CryptoKit
import FluidAudio
import Foundation

struct SpeechModelArgs: Decodable {
  let modelId: String
}

enum SpeechModelID: String, CaseIterable, Sendable {
  case parakeetV3 = "parakeet-v3"
  case parakeetV2 = "parakeet-v2"

  static func parse(_ value: String) throws -> SpeechModelID {
    guard let model = Self(rawValue: value) else {
      throw NativeFailure(message: "Unknown speech model: \(value).")
    }
    return model
  }

  var repository: String {
    switch self {
    case .parakeetV3: return "FluidInference/parakeet-tdt-0.6b-v3-coreml"
    case .parakeetV2: return "FluidInference/parakeet-tdt-0.6b-v2-coreml"
    }
  }

  var version: AsrModelVersion { self == .parakeetV3 ? .v3 : .v2 }
  var jointBundle: String {
    self == .parakeetV3 ? "JointDecisionv3.mlmodelc" : "JointDecision.mlmodelc"
  }
}

struct TranscriptionArgs: Decodable {
  let modelId: String
  let path: String
  let offsetSeconds: Double
  let durationSeconds: Double
}

struct NativeWord: Encodable {
  let text: String
  let start: Double
  let end: Double
}

struct ModelStatus: Encodable {
  let ready: Bool
  let downloading: Bool
  let phase: String
  let modelId: String
  let revision: String
  let downloadedBytes: UInt64
  let totalBytes: UInt64
}

struct ModelManifest: Decodable, Sendable {
  let repository: String
  let revision: String
  let files: [ModelFile]

  var totalBytes: UInt64 { files.reduce(0) { $0 + $1.size } }

  func validate(for modelID: SpeechModelID) throws {
    let bundles = [
      "Encoder.mlmodelc", "Decoder.mlmodelc", "Preprocessor.mlmodelc", modelID.jointBundle,
    ]
    let names = [
      "analytics/coremldata.bin", "coremldata.bin", "metadata.json", "model.mil",
      "weights/weight.bin",
    ]
    let expected = Set(
      bundles.flatMap { bundle in names.map { "\(bundle)/\($0)" } } + ["parakeet_vocab.json"])
    guard repository == modelID.repository,
      revision.range(of: "^[a-f0-9]{40}$", options: .regularExpression) != nil,
      files.count == expected.count, Set(files.map(\.path)) == expected,
      files.allSatisfy({
        $0.size > 0 && $0.sha256.range(of: "^[a-f0-9]{64}$", options: .regularExpression) != nil
      })
    else { throw NativeFailure(message: "Invalid pinned speech model manifest.") }
  }
}

struct ModelFile: Decodable, Sendable {
  let path: String
  let size: UInt64
  let sha256: String
}

final class DownloadProgress: NSObject, URLSessionDownloadDelegate {
  private let lock = NSLock()
  private var completed: UInt64 = 0
  private var current: UInt64 = 0
  private var fileSize: UInt64 = 0

  var bytes: UInt64 {
    lock.lock()
    defer { lock.unlock() }
    return completed + current
  }

  func reset() {
    lock.lock()
    defer { lock.unlock() }
    completed = 0
    current = 0
    fileSize = 0
  }

  func beginFile(size: UInt64) {
    lock.lock()
    defer { lock.unlock() }
    current = 0
    fileSize = size
  }

  func discardCurrentFile() {
    lock.lock()
    defer { lock.unlock() }
    current = 0
    fileSize = 0
  }

  func finishFile() {
    lock.lock()
    defer { lock.unlock() }
    completed += fileSize
    current = 0
    fileSize = 0
  }

  func urlSession(
    _ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64
  ) {
    lock.lock()
    defer { lock.unlock() }
    current = min(fileSize, UInt64(max(0, totalBytesWritten)))
  }

  func urlSession(
    _ session: URLSession, downloadTask: URLSessionDownloadTask,
    didFinishDownloadingTo location: URL
  ) {}
}

actor NativeSpeech {
  private var downloadingModel: SpeechModelID?
  private var verifyingModel: SpeechModelID?
  private var progressModel: SpeechModelID?
  private var transcribing = false
  private var manager: AsrManager?
  private var managerModel: SpeechModelID?
  private var manifests: [SpeechModelID: ModelManifest] = [:]
  private var verified: Set<SpeechModelID> = []
  private let progress = DownloadProgress()
  private let baseDirectory: URL?

  init(baseDirectory: URL? = nil) { self.baseDirectory = baseDirectory }

  private func modelManifest(_ modelID: SpeechModelID) throws -> ModelManifest {
    if let manifest = manifests[modelID] { return manifest }
    let decoded = try JSONDecoder().decode(
      ModelManifest.self, from: PinnedSpeechManifest.data(for: modelID))
    try decoded.validate(for: modelID)
    manifests[modelID] = decoded
    return decoded
  }

  private func directory(_ manifest: ModelManifest) throws -> URL {
    let base =
      try baseDirectory
      ?? FileManager.default.url(
        for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true
      ).appendingPathComponent("Loofah/Models", isDirectory: true)
    return base.appendingPathComponent(manifest.revision, isDirectory: true)
  }

  private func filesReady(_ manifest: ModelManifest) throws -> Bool {
    let root = try directory(manifest)
    guard FileManager.default.fileExists(atPath: root.appendingPathComponent("verified").path)
    else { return false }
    return manifest.files.allSatisfy { file in
      guard
        let attrs = try? FileManager.default.attributesOfItem(
          atPath: root.appendingPathComponent(file.path).path),
        let size = attrs[.size] as? NSNumber
      else { return false }
      return size.uint64Value == file.size
    }
  }

  func status(modelId: String) throws -> ModelStatus {
    let modelID = try SpeechModelID.parse(modelId)
    let manifest = try modelManifest(modelID)
    let ready = try filesReady(manifest)
    let downloading = downloadingModel == modelID
    let phase =
      verifyingModel == modelID
      ? "verifying" : downloading ? "downloading" : ready ? "ready" : "idle"
    return ModelStatus(
      ready: ready, downloading: downloading, phase: phase, modelId: modelID.rawValue,
      revision: manifest.revision,
      downloadedBytes: ready ? manifest.totalBytes : progressModel == modelID ? progress.bytes : 0,
      totalBytes: manifest.totalBytes)
  }

  func download(modelId: String) async throws -> ModelStatus {
    let modelID = try SpeechModelID.parse(modelId)
    guard downloadingModel == nil, !transcribing else {
      throw NativeFailure(message: "The speech model is busy.")
    }
    let manifest = try modelManifest(modelID)
    if try filesReady(manifest) { return try status(modelId: modelId) }
    downloadingModel = modelID
    progressModel = modelID
    progress.reset()
    nativeEvent("model_download_started", message: modelId)
    defer {
      downloadingModel = nil
      verifyingModel = nil
      progress.discardCurrentFile()
    }
    let root = try directory(manifest)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    var resourceValues = URLResourceValues()
    resourceValues.isExcludedFromBackup = true
    var excludedRoot = root
    try excludedRoot.setResourceValues(resourceValues)
    for file in manifest.files {
      try Task.checkCancellation()
      let destination = root.appendingPathComponent(file.path)
      progress.beginFile(size: file.size)
      verifyingModel = modelID
      if (try? await Self.checksum(destination)) == file.sha256 {
        progress.finishFile()
        continue
      }
      verifyingModel = nil
      guard
        let url = URL(
          string:
            "https://huggingface.co/\(manifest.repository)/resolve/\(manifest.revision)/\(file.path)"
        )
      else {
        throw NativeFailure(message: "Invalid speech model URL.")
      }
      let (temporary, response) = try await URLSession.shared.download(
        from: url, delegate: progress)
      defer { try? FileManager.default.removeItem(at: temporary) }
      guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
        throw NativeFailure(message: "The speech model download failed. Try again when online.")
      }
      verifyingModel = modelID
      let attrs = try FileManager.default.attributesOfItem(atPath: temporary.path)
      guard (attrs[.size] as? NSNumber)?.uint64Value == file.size,
        try await Self.checksum(temporary) == file.sha256
      else {
        throw NativeFailure(
          message: "Speech model verification failed for \(file.path). Try downloading again.")
      }
      try FileManager.default.createDirectory(
        at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
      if FileManager.default.fileExists(atPath: destination.path) {
        try FileManager.default.removeItem(at: destination)
      }
      try FileManager.default.moveItem(at: temporary, to: destination)
      progress.finishFile()
    }
    try Data(manifest.revision.utf8).write(
      to: root.appendingPathComponent("verified"), options: .atomic)
    verified.insert(modelID)
    downloadingModel = nil
    verifyingModel = nil
    return try status(modelId: modelId)
  }

  func delete(modelId: String) throws -> ModelStatus {
    let modelID = try SpeechModelID.parse(modelId)
    guard downloadingModel == nil, !transcribing else {
      throw NativeFailure(message: "The speech model is busy.")
    }
    let manifest = try modelManifest(modelID)
    let root = try directory(manifest)
    if managerModel == modelID {
      manager = nil
      managerModel = nil
    }
    verified.remove(modelID)
    if FileManager.default.fileExists(atPath: root.path) {
      try FileManager.default.removeItem(at: root)
    }
    if progressModel == modelID {
      progress.reset()
      progressModel = nil
    }
    return try status(modelId: modelId)
  }

  private static func checksum(_ url: URL) async throws -> String {
    try await Task.detached(priority: .utility) {
      let handle = try FileHandle(forReadingFrom: url)
      defer { try? handle.close() }
      var hash = SHA256()
      while let data = try handle.read(upToCount: 1_048_576), !data.isEmpty {
        hash.update(data: data)
      }
      return hash.finalize().map { String(format: "%02x", $0) }.joined()
    }.value
  }

  private func loadManager(_ modelID: SpeechModelID) async throws -> AsrManager {
    if managerModel == modelID, let manager { return manager }
    manager = nil
    managerModel = nil
    let manifest = try modelManifest(modelID)
    guard try filesReady(manifest) else {
      throw NativeFailure(message: "Download the speech model before transcribing.")
    }
    let root = try directory(manifest)
    verifyingModel = modelID
    defer { verifyingModel = nil }
    if !verified.contains(modelID) {
      for file in manifest.files {
        guard try await Self.checksum(root.appendingPathComponent(file.path)) == file.sha256 else {
          try? FileManager.default.removeItem(at: root.appendingPathComponent("verified"))
          throw NativeFailure(message: "The speech model is damaged. Download it again.")
        }
      }
      verified.insert(modelID)
    }
    // Manual loading forbids fallback network requests and uses only the checked immutable assets.
    ModelHub.offlineMode = true
    let models = try await Task.detached(priority: .utility) {
      let config = MLModelConfiguration()
      config.computeUnits = .cpuAndNeuralEngine
      let cpu = MLModelConfiguration()
      cpu.computeUnits = .cpuOnly
      let vocab = try JSONDecoder().decode(
        [String: String].self,
        from: Data(contentsOf: root.appendingPathComponent("parakeet_vocab.json")))
      var vocabulary: [Int: String] = [:]
      for (key, value) in vocab { if let id = Int(key) { vocabulary[id] = value } }
      let models = AsrModels(
        encoder: try MLModel(
          contentsOf: root.appendingPathComponent("Encoder.mlmodelc"), configuration: config),
        preprocessor: try MLModel(
          contentsOf: root.appendingPathComponent("Preprocessor.mlmodelc"), configuration: cpu),
        decoder: try MLModel(
          contentsOf: root.appendingPathComponent("Decoder.mlmodelc"), configuration: config),
        joint: try MLModel(
          contentsOf: root.appendingPathComponent(modelID.jointBundle), configuration: config),
        configuration: config,
        vocabulary: vocabulary,
        version: modelID.version
      )
      return models
    }.value
    let next = AsrManager(config: ASRConfig(parallelChunkConcurrency: 1), models: models)
    manager = next
    managerModel = modelID
    return next
  }

  func transcribe(_ args: TranscriptionArgs) async throws -> [NativeWord] {
    let modelID = try SpeechModelID.parse(args.modelId)
    guard !transcribing, downloadingModel == nil else {
      throw NativeFailure(message: "The speech model is busy.")
    }
    guard args.offsetSeconds.isFinite, args.durationSeconds.isFinite,
      args.offsetSeconds >= 0, args.durationSeconds > 0, args.durationSeconds <= 30
    else {
      throw NativeFailure(message: "Transcription windows must be between 0 and 30 seconds.")
    }
    transcribing = true
    defer { transcribing = false }
    let samples = try AudioWindow.read(
      path: args.path, offset: args.offsetSeconds, duration: args.durationSeconds)
    guard !samples.isEmpty else { return [] }
    let manager = try await loadManager(modelID)
    var state = try TdtDecoderState()
    let result = try await manager.transcribe(samples, decoderState: &state)
    let duration = Double(samples.count) / 16_000
    let timings = result.tokenTimings ?? []
    guard result.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !timings.isEmpty
    else {
      throw NativeFailure(message: "The speech model did not return word timestamps.")
    }
    return buildWordTimings(from: timings).compactMap { word in
      let start = max(0, min(duration, word.startTime))
      let end = max(start, min(duration, word.endTime))
      return word.word.isEmpty ? nil : NativeWord(text: word.word, start: start, end: end)
    }
  }
}
