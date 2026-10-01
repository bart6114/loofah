import AVFoundation
import Foundation
import SwiftRs
import Tauri
import UIKit

@_silgen_name("loofah_mobile_audio")
func loofahMobileAudio(_ samples: UnsafePointer<Float>, _ count: Int) -> Bool

@_silgen_name("loofah_mobile_event")
func loofahMobileEvent(_ json: UnsafePointer<CChar>)

func nativeEvent(_ type: String, message: String? = nil, sessionID: String? = nil) {
  var event = ["type": type]
  if let message { event["message"] = message }
  if let sessionID { event["session_id"] = sessionID }
  guard let data = try? JSONEncoder().encode(event), let json = String(data: data, encoding: .utf8)
  else { return }
  json.withCString { loofahMobileEvent($0) }
}

struct NativeFailure: LocalizedError {
  let message: String
  var errorDescription: String? { message }
}

class MobileNativePlugin: Plugin {
  let capture = NativeCapture()
  let speech = NativeSpeech()
  var importPicker: AudioImportPicker?
  private var observers: [NSObjectProtocol] = []
  @MainActor private lazy var recordingActivity = RecordingActivity()

  override init() {
    super.init()
    DispatchQueue.main.async { _ = self.recordingActivity }
    capture.onStateChange = { [weak self] state in
      DispatchQueue.main.async { self?.recordingActivity.update(state) }
    }
    let center = NotificationCenter.default
    observers.append(
      center.addObserver(
        forName: Notification.Name("io.loofah.notes.stop-recording"), object: nil, queue: nil
      ) { note in
        guard let sessionID = note.userInfo?["sessionID"] as? String else { return }
        nativeEvent("stop_recording", sessionID: sessionID)
      })
    observers.append(
      center.addObserver(
        forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: nil
      ) { _ in
        nativeEvent("background")
      })
    observers.append(
      center.addObserver(
        forName: UIApplication.willEnterForegroundNotification, object: nil, queue: nil
      ) { _ in
        nativeEvent("foreground")
      })
  }

  deinit {
    observers.forEach { NotificationCenter.default.removeObserver($0) }
  }

  @objc public func startRecording(_ invoke: Invoke) {
    AVAudioApplication.requestRecordPermission { allowed in
      guard allowed else {
        invoke.reject("Microphone permission is required.")
        return
      }
      self.capture.start { result in
        switch result {
        case .success: invoke.resolve()
        case .failure(let error): invoke.reject(error.localizedDescription)
        }
      }
    }
  }

  @objc public func stopRecording(_ invoke: Invoke) {
    Task { @MainActor in
      self.recordingActivity.protectStop()
      self.capture.stop { invoke.resolve() }
    }
  }

  @objc public func beginRecordingActivity(_ invoke: Invoke) throws {
    struct Args: Decodable {
      let sessionId: String
      let title: String
    }
    let args = try invoke.parseArgs(Args.self)
    capture.snapshot { state in
      guard let state else {
        invoke.resolve()
        return
      }
      Task { @MainActor in
        await self.recordingActivity.begin(
          sessionID: args.sessionId, title: args.title, capture: state)
        invoke.resolve()
      }
    }
  }

  @objc public func endRecordingActivity(_ invoke: Invoke) throws {
    struct Args: Decodable { let sessionId: String }
    let args = try invoke.parseArgs(Args.self)
    Task { @MainActor in
      await self.recordingActivity.end(sessionID: args.sessionId)
      invoke.resolve()
    }
  }

  @objc public func modelStatus(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(SpeechModelArgs.self)
    Task {
      do { invoke.resolve(try await speech.status(modelId: args.modelId)) } catch {
        invoke.reject(error.localizedDescription)
      }
    }
  }

  @objc public func downloadModel(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(SpeechModelArgs.self)
    Task {
      do { invoke.resolve(try await speech.download(modelId: args.modelId)) } catch {
        invoke.reject(error.localizedDescription)
      }
    }
  }

  @objc public func deleteModel(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(SpeechModelArgs.self)
    Task {
      do { invoke.resolve(try await speech.delete(modelId: args.modelId)) } catch {
        invoke.reject(error.localizedDescription)
      }
    }
  }

  @objc public func transcribeWindow(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(TranscriptionArgs.self)
    Task {
      do { invoke.resolve(try await speech.transcribe(args)) } catch {
        invoke.reject(error.localizedDescription)
      }
    }
  }

  @objc public func protectRecordingFile(_ invoke: Invoke) throws {
    struct Args: Decodable { let path: String }
    let args = try invoke.parseArgs(Args.self)
    try FileManager.default.setAttributes(
      [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication],
      ofItemAtPath: args.path)
    invoke.resolve()
  }

  @objc public func previewFile(_ invoke: Invoke) throws {
    struct Args: Decodable { let path: String }
    let args = try invoke.parseArgs(Args.self)
    Task { @MainActor in
      do {
        try NativeFiles.preview(path: args.path, from: self.manager.viewController)
        invoke.resolve()
      } catch { invoke.reject(error.localizedDescription) }
    }
  }

  @objc public func audioDuration(_ invoke: Invoke) throws {
    struct Args: Decodable { let path: String }
    let args = try invoke.parseArgs(Args.self)
    let file = try AVAudioFile(forReading: URL(fileURLWithPath: args.path))
    invoke.resolve(Double(file.length) / file.processingFormat.sampleRate)
  }

  @objc public func importAudio(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ImportArgs.self)
    DispatchQueue.main.async {
      guard self.importPicker == nil else {
        invoke.reject("An audio picker is already open.")
        return
      }
      let picker = AudioImportPicker(destination: URL(fileURLWithPath: args.destinationPath)) {
        [weak self]
        result in
        self?.importPicker = nil
        switch result {
        case .success(let imported):
          if let imported { invoke.resolve(imported) } else { invoke.resolve() }
        case .failure(let error): invoke.reject(error.localizedDescription)
        }
      }
      self.importPicker = picker
      picker.present()
    }
  }

  @objc public func selectICloudVault(_ invoke: Invoke) {
    DispatchQueue.main.async { ICloudVaultAccess.shared.select(invoke) }
  }
  @objc public func restoreICloudVault(_ invoke: Invoke) {
    DispatchQueue.main.async { ICloudVaultAccess.shared.restore(invoke) }
  }
  @objc public func disconnectICloudVault(_ invoke: Invoke) {
    DispatchQueue.main.async { ICloudVaultAccess.shared.disconnect(invoke) }
  }
  @objc public func downloadICloudItem(_ invoke: Invoke) {
    do {
      struct Args: Decodable { let relativePath: String }
      let args = try invoke.parseArgs(Args.self)
      DispatchQueue.main.async {
        ICloudVaultAccess.shared.download(invoke, relativePath: args.relativePath)
      }
    } catch { invoke.reject(error.localizedDescription) }
  }

  @objc public func setApiKey(_ invoke: Invoke) throws {
    try NativeSecrets.setApiKey(invoke)
  }

  @objc public func readApiKey(_ invoke: Invoke) throws {
    try NativeSecrets.readApiKey(invoke)
  }
}

@_cdecl("init_plugin_mobile_native")
func initPlugin() -> Plugin { MobileNativePlugin() }
