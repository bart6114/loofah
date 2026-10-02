import AVFoundation
import Foundation

final class CaptureTapGate {
  private let lock = NSLock()
  private var active = true
  private var capturedFrames: UInt64 = 0

  func run(_ body: () -> Void) {
    guard lock.try() else { return }
    defer { lock.unlock() }
    guard active else { return }
    body()
  }

  func runCapturing(onFailure: () -> Void = {}, _ body: () -> AVAudioFrameCount?) {
    run {
      guard let frames = body() else {
        active = false
        onFailure()
        return
      }
      capturedFrames += UInt64(frames)
    }
  }

  func frameCount() -> UInt64 {
    lock.lock()
    defer { lock.unlock() }
    return capturedFrames
  }

  @discardableResult
  func close() -> UInt64 {
    lock.lock()
    defer { lock.unlock() }
    active = false
    return capturedFrames
  }
}

struct RecordingCaptureState {
  let capturedSeconds: Double
  let phase: String
  let message: String?
}

final class NativeCapture {
  private let queue = DispatchQueue(label: "io.loofah.notes.capture", qos: .userInitiated)
  private var engine: AVAudioEngine?
  private var tapGate: CaptureTapGate?
  private var wanted = false
  private var interrupted = false
  private var captureFailed = false
  private var capturedFrames: UInt64 = 0
  private var phase = "stopped"
  private var message: String?
  var onStateChange: ((RecordingCaptureState) -> Void)?
  private var observers: [NSObjectProtocol] = []
  private let outputFormat = AVAudioFormat(
    commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false)!

  init() {
    let center = NotificationCenter.default
    observers.append(
      center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: nil)
      { [weak self] note in
        guard let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
          let type = AVAudioSession.InterruptionType(rawValue: raw)
        else { return }
        self?.queue.async { self?.handleInterruption(type, note: note) }
      })
    observers.append(
      center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: nil) {
        [weak self] note in
        guard let raw = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
          let reason = AVAudioSession.RouteChangeReason(rawValue: raw),
          [.oldDeviceUnavailable, .newDeviceAvailable, .routeConfigurationChange].contains(reason)
        else { return }
        self?.queue.async { self?.restart(reason: "Microphone route changed.") }
      })
    observers.append(
      center.addObserver(forName: .AVAudioEngineConfigurationChange, object: nil, queue: nil) {
        [weak self] note in
        self?.queue.async {
          guard let self, let changed = note.object as? AVAudioEngine,
            self.engine === changed, !changed.isRunning
          else { return }
          self.restart(reason: "The microphone configuration changed.")
        }
      })
    observers.append(
      center.addObserver(
        forName: AVAudioSession.mediaServicesWereResetNotification, object: nil, queue: nil
      ) { [weak self] _ in
        self?.queue.async { self?.restart(reason: "Audio services restarted.") }
      })
  }

  deinit {
    observers.forEach { NotificationCenter.default.removeObserver($0) }
    tapGate?.close()
    engine?.stop()
  }

  func start(completion: @escaping (Result<Void, Error>) -> Void) {
    queue.async {
      guard !self.wanted else {
        completion(.failure(NativeFailure(message: "A recording is already active.")))
        return
      }
      do {
        self.capturedFrames = 0
        self.captureFailed = false
        try self.startEngine()
        self.wanted = true
        self.interrupted = false
        self.publish(phase: "recording")
        completion(.success(()))
      } catch { completion(.failure(error)) }
    }
  }

  func stop(completion: @escaping () -> Void) {
    queue.async {
      self.wanted = false
      self.interrupted = false
      self.captureFailed = false
      self.stopEngine()
      self.publish(phase: "stopping")
      try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
      completion()
    }
  }

  func snapshot(_ completion: @escaping (RecordingCaptureState?) -> Void) {
    queue.async { completion(self.wanted ? self.state() : nil) }
  }

  private func state() -> RecordingCaptureState {
    RecordingCaptureState(
      capturedSeconds: Double(capturedFrames + (tapGate?.frameCount() ?? 0)) / 16_000,
      phase: phase, message: message)
  }

  private func publish(phase: String, message: String? = nil) {
    self.phase = phase
    self.message = message
    onStateChange?(state())
  }

  private func stopEngine() {
    // Closing waits for the last PCM copy so stop completion cannot race a later recording.
    capturedFrames += tapGate?.close() ?? 0
    tapGate = nil
    engine?.stop()
    engine?.inputNode.removeTap(onBus: 0)
    engine = nil
  }

  private func startEngine() throws {
    let session = AVAudioSession.sharedInstance()
    try session.setCategory(.record, mode: .measurement, options: [.allowBluetoothHFP])
    try session.setPreferredSampleRate(16_000)
    try session.setPreferredIOBufferDuration(0.04)
    try session.setActive(true)
    var started = false
    defer {
      if !started { try? session.setActive(false, options: .notifyOthersOnDeactivation) }
    }
    let next = AVAudioEngine()
    let input = next.inputNode
    let format = input.outputFormat(forBus: 0)
    guard format.sampleRate > 0, format.channelCount > 0,
      let converter = AVAudioConverter(from: format, to: outputFormat)
    else {
      throw NativeFailure(message: "No microphone input is available.")
    }
    converter.downmix = true
    let capacity: AVAudioFrameCount = 8192
    guard let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: capacity) else {
      throw NativeFailure(message: "Could not allocate the microphone buffer.")
    }
    let gate = CaptureTapGate()
    input.installTap(onBus: 0, bufferSize: 2048, format: format) { [weak self] buffer, _ in
      var failureMessage =
        "Microphone conversion failed. Stop this recording to preserve its audio."
      gate.runCapturing(onFailure: {
        let message = failureMessage
        self?.queue.async { [weak self] in
          guard let self, self.wanted, self.tapGate === gate else { return }
          self.interrupted = true
          self.captureFailed = true
          self.stopEngine()
          self.publish(phase: "interrupted", message: message)
          nativeEvent("recording_error", message: message)
        }
      }) {
        guard Double(buffer.frameLength) * 16_000 / format.sampleRate + 64 <= Double(capacity)
        else {
          failureMessage =
            "Microphone buffer exceeded its capacity. Stop to preserve the recording."
          return nil
        }
        var supplied = false
        var conversionError: NSError?
        output.frameLength = 0
        let status = converter.convert(to: output, error: &conversionError) { _, status in
          if supplied {
            status.pointee = .noDataNow
            return nil
          }
          supplied = true
          status.pointee = .haveData
          return buffer
        }
        if let conversionError {
          failureMessage = conversionError.localizedDescription
          return nil
        } else if status == .error {
          return nil
        } else if let samples = output.floatChannelData?[0],
          output.frameLength > 0
        {
          // The Rust callback only copies into a bounded queue; no disk or inference work runs on the tap.
          guard loofahMobileAudio(samples, Int(output.frameLength)) else {
            failureMessage = "Audio could not be saved. Stop this recording to preserve its audio."
            return nil
          }
          return output.frameLength
        }
        return 0
      }
    }
    next.prepare()
    do {
      try next.start()
      engine = next
      tapGate = gate
      started = true
    } catch {
      gate.close()
      input.removeTap(onBus: 0)
      throw error
    }
  }

  private func handleInterruption(_ type: AVAudioSession.InterruptionType, note: Notification) {
    guard wanted, !captureFailed else { return }
    switch type {
    case .began:
      interrupted = true
      stopEngine()
      publish(
        phase: "interrupted", message: "The microphone was interrupted by another app or a call.")
      nativeEvent(
        "recording_interrupted", message: "The microphone was interrupted by another app or a call."
      )
    case .ended:
      let raw = note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
      guard AVAudioSession.InterruptionOptions(rawValue: raw).contains(.shouldResume) else {
        publish(
          phase: "interrupted", message: "Recording is paused. Open Loofah to stop and save it.")
        nativeEvent(
          "recording_error",
          message:
            "The system did not allow recording to resume. Stop this recording and start again.")
        return
      }
      interrupted = false
      restart(reason: "The microphone interruption ended.")
    @unknown default: break
    }
  }

  private func restart(reason: String) {
    guard wanted, !interrupted, !captureFailed else { return }
    stopEngine()
    publish(phase: "interrupted", message: reason)
    nativeEvent("recording_interrupted", message: reason)
    do {
      try startEngine()
      publish(phase: "recording")
      nativeEvent("recording_resumed")
    } catch {
      publish(phase: "interrupted", message: error.localizedDescription)
      nativeEvent("recording_error", message: error.localizedDescription)
    }
  }
}
