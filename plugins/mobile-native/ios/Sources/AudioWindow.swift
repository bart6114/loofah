import AVFoundation
import Foundation

struct AudioWindow {
  static func read(path: String, offset: Double, duration: Double) throws -> [Float] {
    let file = try AVAudioFile(
      forReading: URL(fileURLWithPath: path), commonFormat: .pcmFormatFloat32, interleaved: false)
    let source = file.processingFormat
    guard source.sampleRate > 0, source.channelCount > 0,
      offset < Double(file.length) / source.sampleRate,
      let format = AVAudioFormat(
        commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
      let converter = AVAudioConverter(from: source, to: format),
      let input = AVAudioPCMBuffer(pcmFormat: source, frameCapacity: 8192),
      let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8192)
    else {
      throw NativeFailure(message: "The recording format or transcription offset is invalid.")
    }
    converter.downmix = true
    file.framePosition = AVAudioFramePosition(offset * source.sampleRate)
    var remaining = min(
      file.length - file.framePosition, AVAudioFramePosition(duration * source.sampleRate))
    let maximum = Int(duration * 16_000)
    var samples: [Float] = []
    samples.reserveCapacity(maximum)
    var readingError: Error?
    while samples.count < maximum {
      var conversionError: NSError?
      let status = converter.convert(to: output, error: &conversionError) { requested, status in
        guard remaining > 0 else {
          status.pointee = .endOfStream
          return nil
        }
        do {
          let count = AVAudioFrameCount(min(remaining, Int64(min(requested, input.frameCapacity))))
          try file.read(into: input, frameCount: count)
          remaining -= Int64(input.frameLength)
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
      if status == .error { throw NativeFailure(message: "The recording could not be decoded.") }
      if let pointer = output.floatChannelData?[0], output.frameLength > 0 {
        samples.append(
          contentsOf: UnsafeBufferPointer(
            start: pointer, count: min(Int(output.frameLength), maximum - samples.count)))
      }
      if status == .endOfStream { break }
    }
    return samples
  }
}
