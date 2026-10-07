import AVFoundation
import FluidAudio
import Foundation
import SwiftRs

private enum DiarizeBridgeError: LocalizedError {
  case message(String)

  var errorDescription: String? {
    switch self {
    case .message(let message):
      return message
    }
  }
}

private struct DiarizeSegmentPayload: Codable {
  var startMs: Int64
  var endMs: Int64
  var speakerIndex: Int
}

private struct DiarizeRunPayload: Codable {
  var segments: [DiarizeSegmentPayload]
  var error: String?
}

private actor DiarizeBridge {
  static let shared = DiarizeBridge()

  private var diarizer: DiarizerManager?
  private var modelsTask: Task<DiarizerModels, Error>?
  private var state: ModelDownloadPayload?

  func modelDownloadStateJSON() -> String {
    refreshReadyState()
    return encodeJSON(currentState())
  }

  func startModelDownload() {
    refreshReadyState()

    if filesReady(), modelsTask == nil {
      var state = currentState()
      state.status = "ready"
      state.currentFile = nil
      state.error = nil
      self.state = state
      return
    }

    if modelsTask != nil {
      var state = currentState()
      state.status = "downloading"
      self.state = state
      return
    }

    var state = currentState()
    state.status = "downloading"
    state.currentFile = "Preparing speaker detection..."
    state.progressPercent = nil
    state.error = nil
    self.state = state

    let task = Task.detached(priority: .utility) {
      try await DiarizerModels.downloadIfNeeded(progressHandler: { progress in
        Task {
          await DiarizeBridge.shared.updateDownloadProgress(progress)
        }
      })
    }

    modelsTask = task

    Task.detached {
      do {
        let models = try await task.value
        await DiarizeBridge.shared.finishModelLoad(models: models)
      } catch {
        await DiarizeBridge.shared.finishModelLoad(error: error)
      }
    }
  }

  func diarizeJSON(samplesData: Data, sampleRate: Int) async -> String {
    do {
      return await diarizeJSON(
        samples: try decodeFloatSamples(from: samplesData), sampleRate: sampleRate)
    } catch {
      return encodeJSON(DiarizeRunPayload(segments: [], error: error.localizedDescription))
    }
  }

  func diarizeFileJSON(path: String, channel: Int) async -> String {
    do {
      let file = try AVAudioFile(
        forReading: URL(fileURLWithPath: path), commonFormat: .pcmFormatFloat32, interleaved: false)
      let format = file.processingFormat
      guard format.sampleRate == 16000, channel >= 0, channel < Int(format.channelCount) else {
        throw DiarizeBridgeError.message("Invalid speaker detection PCM format or channel.")
      }
      guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8192) else {
        throw DiarizeBridgeError.message("Cannot allocate speaker detection buffer.")
      }
      var samples: [Float] = []
      samples.reserveCapacity(Int(file.length))
      while file.framePosition < file.length {
        try file.read(into: buffer, frameCount: 8192)
        guard buffer.frameLength > 0, let data = buffer.floatChannelData else {
          throw DiarizeBridgeError.message("Truncated speaker detection audio.")
        }
        samples.append(
          contentsOf: UnsafeBufferPointer(start: data[channel], count: Int(buffer.frameLength)))
      }
      return await diarizeJSON(samples: samples, sampleRate: 16000)
    } catch {
      return encodeJSON(DiarizeRunPayload(segments: [], error: error.localizedDescription))
    }
  }

  private func diarizeJSON(samples: [Float], sampleRate: Int) async -> String {
    do {
      guard !samples.isEmpty else {
        return encodeJSON(DiarizeRunPayload(segments: [], error: nil))
      }

      let diarizer = try await ensureDiarizer()
      let result = try diarizer.performCompleteDiarization(samples, sampleRate: sampleRate)

      // FluidAudio speaker ids are opaque strings; remap to stable 0-based
      // indices in order of first appearance on the timeline.
      var indexBySpeaker: [String: Int] = [:]
      let segments = result.segments
        .sorted { $0.startTimeSeconds < $1.startTimeSeconds }
        .map { segment -> DiarizeSegmentPayload in
          let index: Int
          if let existing = indexBySpeaker[segment.speakerId] {
            index = existing
          } else {
            index = indexBySpeaker.count
            indexBySpeaker[segment.speakerId] = index
          }

          return DiarizeSegmentPayload(
            startMs: Int64((Double(segment.startTimeSeconds) * 1000.0).rounded()),
            endMs: Int64((Double(segment.endTimeSeconds) * 1000.0).rounded()),
            speakerIndex: index
          )
        }

      return encodeJSON(DiarizeRunPayload(segments: segments, error: nil))
    } catch {
      return encodeJSON(DiarizeRunPayload(segments: [], error: error.localizedDescription))
    }
  }

  private func ensureDiarizer() async throws -> DiarizerManager {
    if let diarizer {
      return diarizer
    }

    let models: DiarizerModels
    if let task = modelsTask {
      models = try await task.value
    } else {
      guard filesReady() else {
        throw DiarizeBridgeError.message("Speaker detection model is not downloaded.")
      }
      models = try await DiarizerModels.load()
    }

    if let diarizer {
      return diarizer
    }

    let manager = DiarizerManager()
    manager.initialize(models: models)
    diarizer = manager
    return manager
  }

  private func updateDownloadProgress(_ progress: DownloadProgress) {
    var state = currentState()
    state.status = "downloading"
    state.localPath = Self.modelsDirectoryPath()
    state.error = nil
    state.progressPercent = Int(max(0.0, min(1.0, progress.fractionCompleted)) * 100.0)

    switch progress.phase {
    case .compiling(let modelName):
      state.currentFile = "Compiling \(modelName)..."
    case .downloading:
      state.currentFile = "Downloading speaker detection..."
    default:
      state.currentFile = "Preparing speaker detection..."
    }

    self.state = state
  }

  private func finishModelLoad(models: DiarizerModels) {
    modelsTask = nil

    let manager = DiarizerManager()
    manager.initialize(models: models)
    diarizer = manager

    var state = currentState()
    state.localPath = Self.modelsDirectoryPath()
    state.status = "ready"
    state.currentFile = nil
    state.progressPercent = nil
    state.error = nil
    self.state = state
  }

  private func finishModelLoad(error: Error) {
    modelsTask = nil

    var state = currentState()
    state.localPath = Self.modelsDirectoryPath()
    state.status = "error"
    state.currentFile = nil
    state.progressPercent = nil
    state.error = error.localizedDescription
    self.state = state
  }

  private func refreshReadyState() {
    var state = currentState()
    state.localPath = Self.modelsDirectoryPath()

    guard modelsTask == nil else {
      self.state = state
      return
    }

    if filesReady() {
      state.status = "ready"
      state.error = nil
      state.currentFile = nil
      state.progressPercent = nil
    } else if state.status == "ready" {
      state.status = "idle"
      state.currentFile = nil
      state.progressPercent = nil
      state.error = nil
      diarizer = nil
    }

    self.state = state
  }

  private func currentState() -> ModelDownloadPayload {
    if let state {
      return state
    }

    return ModelDownloadPayload(
      status: "idle",
      currentFile: nil,
      progressPercent: nil,
      localPath: Self.modelsDirectoryPath(),
      error: nil
    )
  }

  private func filesReady() -> Bool {
    let directory = DiarizerModels.defaultModelsDirectory()

    return DiarizerModels.requiredModelNames.allSatisfy { name in
      var isDirectory = ObjCBool(false)
      let model = directory.appendingPathComponent(name)
      guard
        Foundation.FileManager().fileExists(
          atPath: model.path, isDirectory: &isDirectory),
        isDirectory.boolValue
      else {
        return false
      }

      return Foundation.FileManager().fileExists(
        atPath: model.appendingPathComponent("coremldata.bin").path
      )
    }
  }

  private static func modelsDirectoryPath() -> String {
    DiarizerModels.defaultModelsDirectory().path
  }
}

@_cdecl("_diarize_model_download_state")
public func _diarize_model_download_state() -> SRString {
  SRString(
    waitForValue {
      await DiarizeBridge.shared.modelDownloadStateJSON()
    })
}

@_cdecl("_diarize_start_model_download")
public func _diarize_start_model_download() -> Bool {
  waitForValue {
    await DiarizeBridge.shared.startModelDownload()
    return true
  }
}

@_cdecl("_diarize_run")
public func _diarize_run(samples: SRData, sampleRateHz: Int) -> SRString {
  SRString(
    waitForValue {
      await DiarizeBridge.shared.diarizeJSON(
        samplesData: Data(samples.toArray()),
        sampleRate: sampleRateHz
      )
    })
}

@_cdecl("_diarize_file")
public func _diarize_file(path: SRString, channel: Int) -> SRString {
  SRString(
    waitForValue {
      await DiarizeBridge.shared.diarizeFileJSON(path: path.toString(), channel: channel)
    })
}
