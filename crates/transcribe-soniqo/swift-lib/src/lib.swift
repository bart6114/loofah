import AudioCommon
import Foundation
import NemotronStreamingASR
import OmnilingualASR
import ParakeetASR
import ParakeetStreamingASR
import SwiftRs

private enum SoniqoBridgeError: LocalizedError {
  case message(String)

  var errorDescription: String? {
    switch self {
    case .message(let message):
      return message
    }
  }
}

private let soniqoFileTranscriptionSampleRate = 16_000
private let parakeetBatchMinimumChunkSeconds = 20.0
private let parakeetBatchMaximumChunkSeconds = 29.5

private enum SpeechModelKind: String, CaseIterable {
  case parakeetStreaming = "soniqo-parakeet-streaming"
  case parakeetBatch = "soniqo-parakeet-batch"
  case nemotron = "soniqo-nemotron-3.5-streaming"
  case omnilingual = "soniqo-omnilingual"

  static func resolve(_ identifier: String) -> Self? {
    Self(rawValue: identifier) ?? Self.allCases.first(where: { $0.repo == identifier })
  }

  var label: String {
    switch self {
    case .parakeetStreaming:
      return "Soniqo Parakeet Streaming"
    case .parakeetBatch:
      return "Soniqo Parakeet Batch"
    case .nemotron:
      return "Nemotron 3.5 (Multilingual)"
    case .omnilingual:
      return "Soniqo Omnilingual"
    }
  }

  var repo: String {
    switch self {
    case .parakeetStreaming:
      return "aufklarer/Parakeet-EOU-120M-CoreML-INT8"
    case .parakeetBatch:
      return "aufklarer/Parakeet-TDT-v3-CoreML-INT8"
    case .nemotron:
      return "aufklarer/Nemotron-3.5-ASR-Streaming-0.6B-CoreML-INT8"
    case .omnilingual:
      return "aufklarer/Omnilingual-ASR-CTC-300M-CoreML-INT8-10s"
    }
  }

  var isStreamingCapable: Bool {
    self == .parakeetStreaming || self == .nemotron
  }

  var fileTranscriptionChunkSeconds: Double? {
    switch self {
    case .parakeetStreaming:
      return nil
    case .parakeetBatch:
      return parakeetBatchMaximumChunkSeconds
    case .nemotron:
      return 25
    case .omnilingual:
      return 35
    }
  }

  var minimumFileTranscriptionChunkSeconds: Double? {
    switch self {
    case .parakeetBatch:
      return parakeetBatchMinimumChunkSeconds
    case .parakeetStreaming, .nemotron, .omnilingual:
      return nil
    }
  }

  var maximumFileTranscriptionChunkSeconds: Double? {
    switch self {
    case .parakeetBatch:
      return parakeetBatchMaximumChunkSeconds
    case .parakeetStreaming, .nemotron, .omnilingual:
      return nil
    }
  }

  func cacheDirectoryURL() throws -> URL {
    try HuggingFaceDownloader.getCacheDirectory(for: repo)
  }

  func cacheDirectoryPath() -> String {
    (try? cacheDirectoryURL().path) ?? ""
  }

  func filesReady() -> Bool {
    guard let directory = try? cacheDirectoryURL() else {
      return false
    }

    switch self {
    case .parakeetStreaming, .parakeetBatch:
      return Self.regularFileExists(at: directory.appendingPathComponent("config.json"))
        && Self.regularFileExists(at: directory.appendingPathComponent("vocab.json"))
        && Self.compiledCoreMLModelReady(at: directory.appendingPathComponent("encoder.mlmodelc"))
        && Self.compiledCoreMLModelReady(at: directory.appendingPathComponent("decoder.mlmodelc"))
        && Self.compiledCoreMLModelReady(at: directory.appendingPathComponent("joint.mlmodelc"))
    case .nemotron:
      return Self.regularFileExists(at: directory.appendingPathComponent("config.json"))
        && Self.regularFileExists(at: directory.appendingPathComponent("vocab.json"))
        && Self.regularFileExists(at: directory.appendingPathComponent("languages.json"))
        && Self.regularFileExists(at: directory.appendingPathComponent("tokenizer.model"))
        && Self.compiledCoreMLModelReady(at: directory.appendingPathComponent("encoder.mlmodelc"))
        && Self.compiledCoreMLModelReady(at: directory.appendingPathComponent("decoder.mlmodelc"))
        && Self.compiledCoreMLModelReady(at: directory.appendingPathComponent("joint.mlmodelc"))
    case .omnilingual:
      return Self.regularFileExists(at: directory.appendingPathComponent("config.json"))
        && Self.regularFileExists(at: directory.appendingPathComponent("tokenizer.model"))
        && Self.directoryContainsRegularFile(
          at: directory.appendingPathComponent("omnilingual-ctc-300m-int8.mlpackage")
        )
    }
  }

  func load(progressHandler: ((Double, String) -> Void)?) async throws -> LoadedSpeechModel {
    let offlineMode = filesReady()

    switch self {
    case .parakeetStreaming:
      return .streaming(
        try await ParakeetStreamingASRModel.fromPretrained(
          modelId: repo,
          progressHandler: progressHandler
        )
      )
    case .parakeetBatch:
      return .parakeetBatch(
        try await ParakeetASRModel.fromPretrained(
          modelId: repo,
          offlineMode: offlineMode,
          progressHandler: progressHandler
        )
      )
    case .nemotron:
      if offlineMode {
        return .nemotron(
          try await NemotronStreamingASRModel.fromLocal(
            bundleDir: cacheDirectoryURL(), computeUnits: .all, progressHandler: progressHandler
          )
        )
      }
      return .nemotron(
        try await NemotronStreamingASRModel.fromPretrained(
          modelId: repo, computeUnits: .all, progressHandler: progressHandler
        )
      )
    case .omnilingual:
      return .omnilingual(
        try await OmnilingualASRModel.fromPretrained(
          modelId: repo,
          offlineMode: offlineMode,
          progressHandler: progressHandler
        )
      )
    }
  }

  private static func regularFileExists(at url: URL) -> Bool {
    var isDirectory = ObjCBool(false)
    return Foundation.FileManager().fileExists(atPath: url.path, isDirectory: &isDirectory)
      && !isDirectory.boolValue
  }

  private static func compiledCoreMLModelReady(at directory: URL) -> Bool {
    var isDirectory = ObjCBool(false)
    guard
      Foundation.FileManager().fileExists(
        atPath: directory.path, isDirectory: &isDirectory),
      isDirectory.boolValue
    else {
      return false
    }

    return regularFileExists(at: directory.appendingPathComponent("model.mil"))
      && directoryContainsRegularFile(at: directory.appendingPathComponent("weights"))
  }

  private static func directoryContainsFile(withExtension pathExtension: String, in directory: URL)
    -> Bool
  {
    guard
      let contents = try? Foundation.FileManager().contentsOfDirectory(
        at: directory,
        includingPropertiesForKeys: [.isRegularFileKey]
      )
    else {
      return false
    }

    return contents.contains { candidate in
      guard
        candidate.pathExtension == pathExtension,
        let values = try? candidate.resourceValues(forKeys: [.isRegularFileKey])
      else {
        return false
      }

      return values.isRegularFile == true
    }
  }

  private static func directoryContainsRegularFile(at directory: URL) -> Bool {
    guard
      let enumerator = Foundation.FileManager().enumerator(
        at: directory,
        includingPropertiesForKeys: [.isRegularFileKey],
        options: [.skipsHiddenFiles]
      )
    else {
      return false
    }

    for case let candidate as URL in enumerator {
      guard let values = try? candidate.resourceValues(forKeys: [.isRegularFileKey]) else {
        continue
      }

      if values.isRegularFile == true {
        return true
      }
    }

    return false
  }
}

private enum LoadedSpeechModel {
  case streaming(ParakeetStreamingASRModel)
  case parakeetBatch(ParakeetASRModel)
  case omnilingual(OmnilingualASRModel)
  case nemotron(NemotronStreamingASRModel)

  func createLiveSession(language: String?) throws -> LiveSession {
    try autoreleasepool {
      switch self {
      case .streaming(let model):
        return .parakeet(try model.createSession())
      case .nemotron(let model):
        return .nemotron(try model.createSession(language: language))
      default:
        throw SoniqoBridgeError.message(
          "The selected model does not support realtime transcription.")
      }
    }
  }

  func transcribe(audio: [Float], sampleRate: Int, language: String?) throws -> String {
    let normalizedLanguage = language?.trimmingCharacters(in: .whitespacesAndNewlines)
    let languageHint = (normalizedLanguage?.isEmpty == false) ? normalizedLanguage : nil

    switch self {
    case .streaming(let model):
      return try model.transcribeAudio(audio, sampleRate: sampleRate)
    case .parakeetBatch(let model):
      return try model.transcribeAudio(audio, sampleRate: sampleRate, language: languageHint)
    case .nemotron(let model):
      return try model.transcribeAudio(
        audio, sampleRate: sampleRate, language: languageHint, padSilence: false)
    case .omnilingual(let model):
      return try model.transcribeAudio(audio, sampleRate: sampleRate)
    }
  }
}

private enum LiveSession {
  case parakeet(ParakeetStreamingASR.StreamingSession)
  case nemotron(NemotronStreamingASR.StreamingSession)

  func pushAudio(_ samples: [Float], source: String) throws -> [LivePartialPayload] {
    switch self {
    case .parakeet(let session):
      return try session.pushAudio(samples).map {
        LivePartialPayload(source: source, text: $0.text, isFinal: $0.isFinal)
      }
    case .nemotron(let session):
      return try session.pushAudio(samples).map {
        LivePartialPayload(source: source, text: $0.text, isFinal: $0.isFinal)
      }
    }
  }

  func finalize(source: String) throws -> [LivePartialPayload] {
    switch self {
    case .parakeet(let session):
      return try session.finalize().map {
        LivePartialPayload(source: source, text: $0.text, isFinal: $0.isFinal)
      }
    case .nemotron(let session):
      return try session.finalize().map {
        LivePartialPayload(source: source, text: $0.text, isFinal: $0.isFinal)
      }
    }
  }
}

private enum TranscriptSource: String, Codable, CaseIterable {
  case microphone
  case system
}

struct ModelDownloadPayload: Codable {
  var status: String
  var currentFile: String?
  var progressPercent: Int?
  var localPath: String
  var error: String?
}

private struct FileTranscriptionPayload: Codable {
  var text: String
  var durationSeconds: Double
  var error: String?
}

private struct LivePartialPayload: Codable {
  var source: String
  var text: String
  var isFinal: Bool
}

private struct LiveAppendPayload: Codable {
  var partials: [LivePartialPayload]
  var error: String?
}

private struct StatusPayload: Codable {
  var running: Bool
  var error: String?
}

func encodeJSON<T: Encodable>(_ value: T) -> String {
  guard let data = try? JSONEncoder().encode(value),
    let string = String(data: data, encoding: .utf8)
  else {
    return "{}"
  }

  return string
}

func waitForValue<T>(_ operation: @escaping () async -> T) -> T {
  let semaphore = DispatchSemaphore(value: 0)
  var result: T!

  Task {
    result = await operation()
    semaphore.signal()
  }

  semaphore.wait()
  return result
}

func decodeFloatSamples(from data: Data) throws -> [Float] {
  let stride = MemoryLayout<Float>.size
  guard data.count.isMultiple(of: stride) else {
    throw SoniqoBridgeError.message("Invalid audio chunk received by Soniqo.")
  }

  let count = data.count / stride
  var samples = [Float]()
  samples.reserveCapacity(count)

  data.withUnsafeBytes { bytes in
    for index in 0..<count {
      let bits = bytes.loadUnaligned(fromByteOffset: index * stride, as: UInt32.self)
      samples.append(Float(bitPattern: UInt32(littleEndian: bits)))
    }
  }

  return samples
}

private actor SoniqoBridge {
  static let shared = SoniqoBridge()

  private var loadedModels: [SpeechModelKind: LoadedSpeechModel] = [:]
  private var modelTasks: [SpeechModelKind: Task<LoadedSpeechModel, Error>] = [:]
  private var downloadStates: [SpeechModelKind: ModelDownloadPayload] = [:]
  private var activeStreamingSessions: [TranscriptSource: LiveSession] = [:]
  private var activeStreamingKind: SpeechModelKind?
  private var activeStreamingLanguage: String?

  func cacheDirectory(modelId: String) -> String {
    guard let kind = SpeechModelKind.resolve(modelId) else {
      return ""
    }

    refreshReadyState(for: kind)
    return kind.cacheDirectoryPath()
  }

  func modelDownloadStateJSON(modelId: String) -> String {
    guard let kind = SpeechModelKind.resolve(modelId) else {
      return encodeJSON(
        ModelDownloadPayload(
          status: "error",
          currentFile: nil,
          progressPercent: nil,
          localPath: "",
          error: "Unsupported Soniqo model."
        )
      )
    }

    refreshReadyState(for: kind)
    return encodeJSON(downloadState(for: kind))
  }

  func startModelDownload(modelId: String) {
    guard let kind = SpeechModelKind.resolve(modelId) else {
      return
    }

    refreshReadyState(for: kind)
    if kind.filesReady(), modelTasks[kind] == nil {
      var state = downloadState(for: kind)
      state.status = "ready"
      state.currentFile = nil
      state.error = nil
      downloadStates[kind] = state
      return
    }

    if modelTasks[kind] != nil {
      var state = downloadState(for: kind)
      state.status = "downloading"
      downloadStates[kind] = state
      return
    }

    var state = downloadState(for: kind)
    state.status = "downloading"
    state.currentFile = "Preparing \(kind.label)..."
    state.progressPercent = nil
    state.error = nil
    downloadStates[kind] = state

    let task = Task.detached(priority: .utility) {
      try await kind.load { fraction, status in
        Task {
          await SoniqoBridge.shared.updateDownloadProgress(
            kind: kind,
            fraction: fraction,
            status: status
          )
        }
      }
    }

    modelTasks[kind] = task

    Task.detached {
      do {
        let model = try await task.value
        await SoniqoBridge.shared.finishModelLoad(kind: kind, model: model)
      } catch {
        await SoniqoBridge.shared.finishModelLoad(kind: kind, error: error)
      }
    }
  }

  func resetModel(modelId: String) {
    guard let kind = SpeechModelKind.resolve(modelId) else {
      return
    }

    loadedModels[kind] = nil
    modelTasks[kind] = nil
    refreshReadyState(for: kind)

    var state = downloadState(for: kind)
    if state.status != "ready" {
      state.status = "idle"
    }
    state.currentFile = nil
    state.progressPercent = nil
    state.error = nil
    downloadStates[kind] = state
  }

  func startLiveJSON(modelId: String, language: String) async -> String {
    do {
      guard let kind = SpeechModelKind.resolve(modelId) else {
        throw SoniqoBridgeError.message("Unsupported Soniqo model: \(modelId)")
      }
      guard kind.isStreamingCapable else {
        throw SoniqoBridgeError.message("\(kind.label) does not support realtime transcription.")
      }

      let model = try await ensureModelLoaded(kind)
      let hint = language.trimmingCharacters(in: .whitespacesAndNewlines)
      let languageHint = hint.isEmpty ? nil : hint
      activeStreamingSessions = [
        .microphone: try model.createLiveSession(language: languageHint),
        .system: try model.createLiveSession(language: languageHint),
      ]
      activeStreamingKind = kind
      activeStreamingLanguage = languageHint
      return encodeJSON(StatusPayload(running: true, error: nil))
    } catch {
      activeStreamingSessions = [:]
      return encodeJSON(StatusPayload(running: false, error: error.localizedDescription))
    }
  }

  func stopLiveJSON() -> String {
    activeStreamingSessions = [:]
    activeStreamingKind = nil
    activeStreamingLanguage = nil
    return encodeJSON(StatusPayload(running: false, error: nil))
  }

  func appendLiveJSON(source: String, samplesData: Data) -> String {
    do {
      guard let transcriptSource = TranscriptSource(rawValue: source) else {
        throw SoniqoBridgeError.message("Unsupported Soniqo transcript source: \(source)")
      }
      guard let session = activeStreamingSessions[transcriptSource] else {
        throw SoniqoBridgeError.message("No active Soniqo transcription session.")
      }

      let samples = try decodeFloatSamples(from: samplesData)
      let partials = try autoreleasepool {
        try session.pushAudio(samples, source: transcriptSource.rawValue)
      }
      return encodeJSON(LiveAppendPayload(partials: partials, error: nil))
    } catch {
      return encodeJSON(LiveAppendPayload(partials: [], error: error.localizedDescription))
    }
  }

  func finalizeLiveJSON(source: String) -> String {
    do {
      guard let transcriptSource = TranscriptSource(rawValue: source) else {
        throw SoniqoBridgeError.message("Unsupported Soniqo transcript source: \(source)")
      }
      guard let session = activeStreamingSessions[transcriptSource] else {
        throw SoniqoBridgeError.message("No active Soniqo transcription session.")
      }

      let partials = try autoreleasepool {
        try session.finalize(source: transcriptSource.rawValue)
      }
      if activeStreamingKind == .nemotron, let model = loadedModels[.nemotron] {
        activeStreamingSessions[transcriptSource] = try model.createLiveSession(
          language: activeStreamingLanguage
        )
      }
      return encodeJSON(LiveAppendPayload(partials: partials, error: nil))
    } catch {
      return encodeJSON(LiveAppendPayload(partials: [], error: error.localizedDescription))
    }
  }

  func transcribeAudioFileJSON(modelId: String, audioPath: String, language: String) async -> String
  {
    do {
      guard let kind = SpeechModelKind.resolve(modelId) else {
        throw SoniqoBridgeError.message("Unsupported Soniqo model: \(modelId)")
      }

      let trimmedLanguage = language.trimmingCharacters(in: .whitespacesAndNewlines)
      let url = URL(fileURLWithPath: audioPath)
      let audio = try AudioFileLoader.load(
        url: url,
        targetSampleRate: soniqoFileTranscriptionSampleRate
      )
      let model = try await ensureModelLoaded(kind)
      let text = try await transcribeFileAudio(
        model: model,
        kind: kind,
        audio: audio,
        sampleRate: soniqoFileTranscriptionSampleRate,
        language: trimmedLanguage.isEmpty ? nil : trimmedLanguage
      )

      return encodeJSON(
        FileTranscriptionPayload(
          text: text,
          durationSeconds: Double(audio.count) / Double(soniqoFileTranscriptionSampleRate),
          error: nil
        )
      )
    } catch {
      return encodeJSON(
        FileTranscriptionPayload(
          text: "",
          durationSeconds: 0,
          error: error.localizedDescription
        )
      )
    }
  }

  private func transcribeFileAudio(
    model: LoadedSpeechModel,
    kind: SpeechModelKind,
    audio: [Float],
    sampleRate: Int,
    language: String?
  ) async throws -> String {
    guard !audio.isEmpty else {
      return ""
    }

    guard let chunkSeconds = kind.fileTranscriptionChunkSeconds else {
      return try await transcribeFileAudioChunk(
        model: model,
        kind: kind,
        audio: audio,
        sampleRate: sampleRate,
        language: language
      )
    }

    let chunkSampleCount = max(sampleRate, Int((Double(sampleRate) * chunkSeconds).rounded(.up)))
    let minimumTrailingSamples =
      kind.minimumFileTranscriptionChunkSeconds.map {
        max(sampleRate, Int((Double(sampleRate) * $0).rounded(.up)))
      } ?? 0
    let maximumChunkSamples =
      kind.maximumFileTranscriptionChunkSeconds.map {
        max(chunkSampleCount, Int((Double(sampleRate) * $0).rounded(.up)))
      } ?? chunkSampleCount
    let ranges = fileTranscriptionChunkRanges(
      sampleCount: audio.count,
      preferredChunkSamples: chunkSampleCount,
      minimumTrailingSamples: minimumTrailingSamples,
      maximumChunkSamples: maximumChunkSamples
    )

    guard ranges.count > 1 else {
      return try await transcribeFileAudioChunk(
        model: model,
        kind: kind,
        audio: audio,
        sampleRate: sampleRate,
        language: language
      )
    }

    var chunks: [String] = []

    for range in ranges {
      let text = try await transcribeFileAudioChunk(
        model: model,
        kind: kind,
        audio: Array(audio[range]),
        sampleRate: sampleRate,
        language: language
      )
      let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
      if !trimmed.isEmpty {
        chunks.append(trimmed)
      }
    }

    return chunks.joined(separator: " ")
  }

  private func transcribeFileAudioChunk(
    model: LoadedSpeechModel,
    kind: SpeechModelKind,
    audio: [Float],
    sampleRate: Int,
    language: String?
  ) async throws -> String {
    let normalizedAudio = normalizedFileTranscriptionAudio(
      kind: kind,
      audio: audio,
      sampleRate: sampleRate
    )
    if case .nemotron(let nemotron) = model {
      let session = try autoreleasepool { try nemotron.createSession(language: language) }
      let chunkSamples = nemotron.config.streaming.melFrames * nemotron.config.hopLength
      var text = ""
      for start in stride(from: 0, to: normalizedAudio.count, by: chunkSamples) {
        try Task.checkCancellation()
        let end = min(start + chunkSamples, normalizedAudio.count)
        let partials = try autoreleasepool {
          try session.pushAudio(Array(normalizedAudio[start..<end]))
        }
        text = partials.last?.text ?? text
        // Interleave live channels with file inference on the same non-thread-safe model.
        await Task.yield()
      }
      return try autoreleasepool { try session.finalize().last?.text ?? text }
    }
    return try autoreleasepool {
      try model.transcribe(audio: normalizedAudio, sampleRate: sampleRate, language: language)
    }
  }

  private func normalizedFileTranscriptionAudio(
    kind: SpeechModelKind,
    audio: [Float],
    sampleRate: Int
  ) -> [Float] {
    guard kind == .parakeetBatch else {
      return audio
    }

    let minimumSamples = max(
      sampleRate,
      Int((Double(sampleRate) * parakeetBatchMinimumChunkSeconds).rounded(.up))
    )
    guard audio.count < minimumSamples else {
      return audio
    }

    var padded = audio
    padded.append(contentsOf: repeatElement(Float.zero, count: minimumSamples - audio.count))
    return padded
  }

  private func fileTranscriptionChunkRanges(
    sampleCount: Int,
    preferredChunkSamples: Int,
    minimumTrailingSamples: Int,
    maximumChunkSamples: Int
  ) -> [Range<Int>] {
    guard sampleCount > preferredChunkSamples else {
      return [0..<sampleCount]
    }

    var ranges: [Range<Int>] = []
    var start = 0

    while start < sampleCount {
      let end = min(sampleCount, start + preferredChunkSamples)
      ranges.append(start..<end)
      start = end
    }

    guard minimumTrailingSamples > 0, ranges.count >= 2, let trailingRange = ranges.last else {
      return ranges
    }

    let trailingSamples = trailingRange.upperBound - trailingRange.lowerBound
    guard trailingSamples < minimumTrailingSamples else {
      return ranges
    }

    let previousIndex = ranges.count - 2
    let previousRange = ranges[previousIndex]
    let mergedSamples = trailingRange.upperBound - previousRange.lowerBound
    guard mergedSamples <= maximumChunkSamples else {
      return ranges
    }

    ranges.removeLast()
    ranges[previousIndex] = previousRange.lowerBound..<trailingRange.upperBound
    return ranges
  }

  private func ensureModelLoaded(_ kind: SpeechModelKind) async throws -> LoadedSpeechModel {
    refreshReadyState(for: kind)

    if let model = loadedModels[kind] {
      return model
    }

    if let task = modelTasks[kind] {
      let loaded = try await task.value
      loadedModels[kind] = loaded
      return loaded
    }

    let loaded = try await kind.load(progressHandler: nil)
    loadedModels[kind] = loaded
    refreshReadyState(for: kind)
    return loaded
  }

  private func updateDownloadProgress(kind: SpeechModelKind, fraction: Double, status: String) {
    var state = downloadState(for: kind)
    state.status = "downloading"
    state.localPath = kind.cacheDirectoryPath()
    state.error = nil

    let percent = Int(max(0.0, min(1.0, fraction)) * 100.0)
    let statusText = status.trimmingCharacters(in: .whitespacesAndNewlines)
    state.progressPercent = percent
    state.currentFile = statusText.isEmpty ? "Preparing \(kind.label)..." : statusText
    downloadStates[kind] = state
  }

  private func finishModelLoad(kind: SpeechModelKind, model: LoadedSpeechModel) {
    loadedModels[kind] = model
    modelTasks[kind] = nil

    var state = downloadState(for: kind)
    state.localPath = kind.cacheDirectoryPath()
    state.status = "ready"
    state.currentFile = nil
    state.progressPercent = nil
    state.error = nil
    downloadStates[kind] = state
  }

  private func finishModelLoad(kind: SpeechModelKind, error: Error) {
    modelTasks[kind] = nil

    var state = downloadState(for: kind)
    state.localPath = kind.cacheDirectoryPath()
    state.status = "error"
    state.currentFile = nil
    state.progressPercent = nil
    state.error = error.localizedDescription
    downloadStates[kind] = state
  }

  private func refreshReadyState(for kind: SpeechModelKind) {
    var state = downloadState(for: kind)
    state.localPath = kind.cacheDirectoryPath()

    guard modelTasks[kind] == nil else {
      downloadStates[kind] = state
      return
    }

    if kind.filesReady() {
      state.status = "ready"
      state.error = nil
      state.currentFile = nil
      state.progressPercent = nil
    } else if state.status == "ready" {
      state.status = "idle"
      state.currentFile = nil
      state.progressPercent = nil
      state.error = nil
      loadedModels[kind] = nil
    } else if state.localPath.isEmpty {
      state.status = "idle"
    }

    downloadStates[kind] = state
  }

  private func downloadState(for kind: SpeechModelKind) -> ModelDownloadPayload {
    if let state = downloadStates[kind] {
      return state
    }

    return ModelDownloadPayload(
      status: "idle",
      currentFile: nil,
      progressPercent: nil,
      localPath: kind.cacheDirectoryPath(),
      error: nil
    )
  }
}

@_cdecl("_soniqo_model_cache_dir")
public func _soniqo_model_cache_dir(modelId: SRString) -> SRString {
  SRString(
    waitForValue {
      await SoniqoBridge.shared.cacheDirectory(modelId: modelId.toString())
    })
}

@_cdecl("_soniqo_model_download_state")
public func _soniqo_model_download_state(modelId: SRString) -> SRString {
  SRString(
    waitForValue {
      await SoniqoBridge.shared.modelDownloadStateJSON(modelId: modelId.toString())
    })
}

@_cdecl("_soniqo_model_start_download")
public func _soniqo_model_start_download(modelId: SRString) -> Bool {
  waitForValue {
    await SoniqoBridge.shared.startModelDownload(modelId: modelId.toString())
    return true
  }
}

@_cdecl("_soniqo_model_reset")
public func _soniqo_model_reset(modelId: SRString) -> Bool {
  waitForValue {
    await SoniqoBridge.shared.resetModel(modelId: modelId.toString())
    return true
  }
}

@_cdecl("_soniqo_transcribe_audio_file")
public func _soniqo_transcribe_audio_file(
  modelId: SRString,
  audioPath: SRString,
  language: SRString
) -> SRString {
  SRString(
    waitForValue {
      await SoniqoBridge.shared.transcribeAudioFileJSON(
        modelId: modelId.toString(),
        audioPath: audioPath.toString(),
        language: language.toString()
      )
    })
}

@_cdecl("_soniqo_live_start")
public func _soniqo_live_start(modelId: SRString, language: SRString) -> SRString {
  SRString(
    waitForValue {
      await SoniqoBridge.shared.startLiveJSON(
        modelId: modelId.toString(), language: language.toString())
    })
}

@_cdecl("_soniqo_live_append")
public func _soniqo_live_append(source: SRString, samples: SRData) -> SRString {
  SRString(
    waitForValue {
      await SoniqoBridge.shared.appendLiveJSON(
        source: source.toString(),
        samplesData: Data(samples.toArray())
      )
    })
}

@_cdecl("_soniqo_live_finalize")
public func _soniqo_live_finalize(source: SRString) -> SRString {
  SRString(
    waitForValue {
      await SoniqoBridge.shared.finalizeLiveJSON(source: source.toString())
    })
}

@_cdecl("_soniqo_live_stop")
public func _soniqo_live_stop() -> SRString {
  SRString(waitForValue { await SoniqoBridge.shared.stopLiveJSON() })
}
