import AVFoundation
import Foundation
import Tauri
import UIKit
import UniformTypeIdentifiers

struct ImportArgs: Decodable { let destinationPath: String }
struct ImportedAudio: Encodable {
  let path: String
  let durationSeconds: Double
  let title: String
}

final class AudioImportPicker: NSObject, UIDocumentPickerDelegate {
  private let destination: URL
  private var completion: ((Result<ImportedAudio?, Error>) -> Void)?
  private var finished = false

  init(destination: URL, completion: @escaping (Result<ImportedAudio?, Error>) -> Void) {
    self.destination = destination
    self.completion = completion
  }

  func present() {
    guard
      let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first(
        where: { $0.activationState == .foregroundActive }),
      var controller = scene.windows.first(where: { $0.isKeyWindow })?.rootViewController
    else {
      finish(.failure(NativeFailure(message: "The file picker needs a visible app window.")))
      return
    }
    while let presented = controller.presentedViewController { controller = presented }
    let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.audio], asCopy: false)
    picker.delegate = self
    picker.allowsMultipleSelection = false
    controller.present(picker, animated: true)
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    finish(.success(nil))
  }

  func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL])
  {
    guard let source = urls.first else {
      finish(.failure(NativeFailure(message: "No audio file was selected.")))
      return
    }
    DispatchQueue.global(qos: .utility).async {
      let scoped = source.startAccessingSecurityScopedResource()
      defer { if scoped { source.stopAccessingSecurityScopedResource() } }
      var coordinationError: NSError?
      var result: Result<ImportedAudio?, Error> = .failure(
        NativeFailure(message: "Could not read the selected audio file."))
      NSFileCoordinator().coordinate(readingItemAt: source, options: [], error: &coordinationError)
      { coordinated in
        result = Result { try Self.decode(coordinated, to: self.destination) }
      }
      if let coordinationError { result = .failure(coordinationError) }
      let completed = result
      DispatchQueue.main.async { self.finish(completed) }
    }
  }

  private func finish(_ result: Result<ImportedAudio?, Error>) {
    guard !finished else { return }
    finished = true
    let callback = completion
    completion = nil
    callback?(result)
  }

  static func decode(_ source: URL, to destination: URL) throws -> ImportedAudio {
    guard !FileManager.default.fileExists(atPath: destination.path) else {
      throw NativeFailure(message: "The import destination already exists.")
    }
    let inputFile = try AVAudioFile(
      forReading: source, commonFormat: .pcmFormatFloat32, interleaved: false)
    guard inputFile.processingFormat.sampleRate > 0, inputFile.processingFormat.channelCount > 0,
      let format = AVAudioFormat(
        commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
      let converter = AVAudioConverter(from: inputFile.processingFormat, to: format),
      let input = AVAudioPCMBuffer(pcmFormat: inputFile.processingFormat, frameCapacity: 8192),
      let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8192)
    else {
      throw NativeFailure(message: "The selected audio format cannot be decoded.")
    }
    converter.downmix = true
    let settings: [String: Any] = [
      AVFormatIDKey: kAudioFormatLinearPCM,
      AVSampleRateKey: 16_000,
      AVNumberOfChannelsKey: 1,
      AVLinearPCMBitDepthKey: 16,
      AVLinearPCMIsFloatKey: false,
      AVLinearPCMIsBigEndianKey: false,
      AVLinearPCMIsNonInterleaved: false,
    ]
    var succeeded = false
    defer { if !succeeded { try? FileManager.default.removeItem(at: destination) } }
    let outputFile = try AVAudioFile(
      forWriting: destination, settings: settings, commonFormat: .pcmFormatFloat32,
      interleaved: false)
    var frames: Int64 = 0
    var readingError: Error?
    while true {
      var conversionError: NSError?
      let status = converter.convert(to: output, error: &conversionError) { requested, status in
        let remaining = inputFile.length - inputFile.framePosition
        guard remaining > 0 else {
          status.pointee = .endOfStream
          return nil
        }
        do {
          let count = AVAudioFrameCount(min(remaining, Int64(min(requested, input.frameCapacity))))
          try inputFile.read(into: input, frameCount: count)
          status.pointee = input.frameLength == 0 ? .endOfStream : .haveData
          return input.frameLength == 0 ? nil : input
        } catch {
          readingError = error
          status.pointee = .endOfStream
          return nil
        }
      }
      if let readingError { throw readingError }
      if let conversionError { throw conversionError }
      if status == .error { throw NativeFailure(message: "Audio decoding failed.") }
      if output.frameLength > 0 {
        try outputFile.write(from: output)
        frames += Int64(output.frameLength)
      }
      if status == .endOfStream { break }
    }
    guard frames > 0 else { throw NativeFailure(message: "The selected file has no audio.") }
    succeeded = true
    return ImportedAudio(
      path: destination.path, durationSeconds: Double(frames) / 16_000,
      title: source.deletingPathExtension().lastPathComponent)
  }
}
