import AVFoundation
import ActivityKit
import Foundation
import OSLog
import UIKit

struct LocalCapture: Codable, Identifiable {
  var capture: Capture
  var account: Data?
  var queued: Bool? = nil
  var receipt: CaptureReceipt?

  var id: UUID { capture.id }
  var isQueued: Bool { queued == true || receipt != nil }

  var title: String {
    if let title = capture.customTitle { return title }
    let firstLine = capture.note.split(maxSplits: 1, whereSeparator: \.isNewline).first?
      .trimmingCharacters(in: .whitespaces)
    if let firstLine, !firstLine.isEmpty { return String(firstLine.prefix(80)) }
    return capture.recording ? "Recording" : "Note"
  }
}

enum CaptureProblem: Equatable {
  case microphone
  case operation(String)

  var title: String {
    switch self {
    case .microphone: "Microphone access"
    case .operation: "Capture"
    }
  }

  var message: String {
    switch self {
    case .microphone: "Allow microphone access for Loofah in Settings to record."
    case .operation(let message): message
    }
  }
}

enum CaptureDeliveryStatus: Equatable {
  case waiting, syncing, unavailable, confirmAccount, originalAccount, cloudStorage, deviceStorage

  var title: String {
    switch self {
    case .waiting: "Waiting for sync"
    case .syncing: "Syncing to Mac"
    case .unavailable: "iCloud unavailable"
    case .confirmAccount: "Confirm Apple Account"
    case .originalAccount: "Original account required"
    case .cloudStorage: "iCloud storage full"
    case .deviceStorage: "Storage full"
    }
  }

  var detail: String? {
    switch self {
    case .waiting, .syncing: nil
    case .unavailable: "Turn on iCloud Drive to send this capture."
    case .confirmAccount: "This capture was made while signed out."
    case .originalAccount: "Sign in to the Apple Account used to create this capture."
    case .cloudStorage: "Free up iCloud storage to send this capture."
    case .deviceStorage: "Free up storage on this iPhone to continue."
    }
  }
}

private struct PendingAck: Codable {
  let account: Data
  let receipt: CaptureReceipt
}

@MainActor
final class CaptureStore: NSObject, ObservableObject, AVAudioRecorderDelegate {
  static let shared = CaptureStore()
  private nonisolated static let audioLog = Logger(
    subsystem: "io.loofah.notes", category: "recording")

  @Published private(set) var captures: [LocalCapture] = []
  @Published private(set) var recording: RecordingSession?
  @Published private(set) var busy = false
  @Published private var finishing: UUID?
  @Published var problem: CaptureProblem?
  @Published private var recovering: Set<UUID> = []
  @Published private var preparingHandoff: Set<UUID> = []
  @Published private var cloudIssue: CaptureDeliveryStatus?

  private let root: URL
  private let worker = DispatchQueue(label: "io.loofah.capture.inbox", qos: .utility)
  private var observer: CaptureInboxObserver?
  private var timer: Timer?
  private var recorder: AVAudioRecorder?
  private var audioObservers: [NSObjectProtocol] = []
  private var activity: Activity<CaptureRecordingAttributes>?
  private var saveAssertion = UIBackgroundTaskIdentifier.invalid
  private var cloudSyncRunning = false
  private var cloudSyncRequested = false
  private var syncCompletions: [() -> Void] = []

  override init() {
    root = URL.applicationSupportDirectory.appendingPathComponent("Captures", isDirectory: true)
    super.init()
    do {
      try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
      var url = root
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      try url.setResourceValues(values)
    } catch {
      self.problem = .operation(error.localizedDescription)
    }
    reload()
    let orphanedActivities = Activity<CaptureRecordingAttributes>.activities
    Task {
      for orphan in orphanedActivities { await orphan.end(nil, dismissalPolicy: .immediate) }
    }
    observer = CaptureInboxObserver { [weak self] in
      Task { @MainActor in self?.sync() }
    }
    timer = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in
      Task { @MainActor in self?.sync() }
    }
    recoverRecordings()
    sync()
  }

  func create(recording wantsAudio: Bool) async -> UUID? {
    guard !busy, recording == nil else { return nil }
    busy = true
    defer { busy = false }
    if wantsAudio, !(await AVAudioApplication.requestRecordPermission()) {
      problem = .microphone
      return nil
    }
    let capture = Capture(id: UUID(), createdAt: Date(), note: "", recording: wantsAudio)
    let directory = folder(capture.id)
    do {
      let account = try CaptureInbox.account().map(CaptureInbox.archiveAccount)
      try FileManager.default.createDirectory(
        at: directory, withIntermediateDirectories: false,
        attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
      let local = LocalCapture(capture: capture, account: account)
      try Self.save(local, in: directory)
      if wantsAudio {
        let current = RecordingSession(id: capture.id, startedAt: capture.createdAt)
        observeAudioSession(current.observationID)
        try startRecorder(at: directory.appendingPathComponent(CapturePackage.audio))
        recording = current
        startActivity(for: capture)
      }
      captures.insert(local, at: 0)
      return capture.id
    } catch {
      try? FileManager.default.removeItem(at: directory)
      if wantsAudio { removeAudioObservers() }
      self.problem = .operation(error.localizedDescription)
      return nil
    }
  }

  func edit<Value>(_ id: UUID, _ field: WritableKeyPath<Capture, Value>, _ value: Value) {
    guard !busy, !isFinishing(id),
      let index = captures.firstIndex(where: { $0.id == id && canEdit($0) })
    else {
      return
    }
    var local = captures[index]
    local.capture[keyPath: field] = value
    do {
      try Self.save(local, in: folder(id))
      captures[index] = local
    } catch {
      self.problem = .operation(error.localizedDescription)
    }
  }

  func discardEmptyNote(_ id: UUID) {
    guard let index = captures.firstIndex(where: { $0.id == id }),
      !captures[index].isQueued, !captures[index].capture.recording,
      !captures[index].capture.hasText
    else { return }
    do {
      try FileManager.default.removeItem(at: folder(id))
      captures.remove(at: index)
    } catch {
      problem = .operation(error.localizedDescription)
    }
  }

  func finish(_ id: UUID) async -> Bool {
    guard !busy, !recovering.contains(id),
      let local = captures.first(where: { $0.id == id && !$0.isQueued })
    else {
      return false
    }
    busy = true
    finishing = id
    saveAssertion = UIApplication.shared.beginBackgroundTask(withName: "Save capture") {
      [weak self] in self?.endSaveAssertion()
    }
    defer {
      finishing = nil
      busy = false
      endSaveAssertion()
    }
    let current = recording.flatMap { $0.id == id ? $0 : nil }
    let stoppingActivity = current == nil ? nil : activity
    let savingAt = Date()
    let finishedAt =
      current != nil || !local.capture.recording ? current?.pausedAt ?? savingAt : nil
    if current != nil {
      recording = nil
      removeAudioObservers()
      closeRecorder(deactivate: true)
      activity = nil
    }
    if let stoppingActivity {
      await stoppingActivity.update(
        ActivityContent(
          state: .init(
            savingAt: current?.pausedAt ?? savingAt, timerStartedAt: current?.timerStartedAt),
          staleDate: nil),
        timestamp: savingAt)
    }
    do {
      try await finalize(local, stoppedAt: finishedAt, priority: .userInitiated)
      CaptureAppDelegate.scheduleHandoff()
      sync()
      await stoppingActivity?.end(nil, dismissalPolicy: .immediate)
      return true
    } catch {
      await stoppingActivity?.end(nil, dismissalPolicy: .immediate)
      self.problem = .operation(error.localizedDescription)
      return false
    }
  }

  func isFinishing(_ id: UUID) -> Bool {
    finishing == id || recovering.contains(id)
  }

  func canEdit(_ item: LocalCapture) -> Bool {
    item.receipt == nil && !preparingHandoff.contains(item.id)
  }

  private func recoverRecordings() {
    let drafts = captures.filter { $0.capture.recording && !$0.isQueued }
    guard !drafts.isEmpty else { return }
    recovering = Set(drafts.map(\.id))
    Task {
      for draft in drafts {
        do {
          try await finalize(draft, stoppedAt: nil, priority: .utility)
        } catch {
          problem = .operation("Couldn’t finish a recording. It’s still on this iPhone.")
        }
        recovering.remove(draft.id)
      }
      CaptureAppDelegate.scheduleHandoff()
      sync()
    }
  }

  private func finalize(_ draft: LocalCapture, stoppedAt: Date?, priority: TaskPriority)
    async throws
  {
    guard let initialIndex = captures.firstIndex(where: { $0.id == draft.id }) else { return }
    let directory = folder(draft.id)
    var checkpoint = draft
    if checkpoint.capture.finishedAt == nil {
      let lastAudioWrite = stoppedAt == nil ? try RecordingWAV.lastWrite(in: directory) : nil
      checkpoint.capture.finishedAt = max(
        stoppedAt ?? lastAudioWrite ?? draft.capture.createdAt, draft.capture.createdAt)
      captures[initialIndex].capture.finishedAt = checkpoint.capture.finishedAt
    }
    let finished = try await Task.detached(priority: priority) { [checkpoint] () -> LocalCapture? in
      var local = checkpoint
      // Persist the end time before repairing the file, which changes its modification date.
      try Self.save(local, in: directory)
      if local.capture.recording {
        let frames = try RecordingWAV.finalizeCapture(in: directory)
        Self.audioLog.info("Finalized recording with \(frames, privacy: .public) frames")
        if frames == 0 {
          local.capture = Capture(
            id: local.id, createdAt: local.capture.createdAt, finishedAt: local.capture.finishedAt,
            title: local.capture.title, note: local.capture.note, recording: false)
        }
      }
      if !local.capture.recording, !local.capture.hasText {
        try FileManager.default.removeItem(at: directory)
        return nil
      }
      local.queued = true
      try Self.save(local, in: directory)
      if local.capture.recording { RecordingWAV.removeSegments(in: directory) }
      return local
    }.value
    guard let index = captures.firstIndex(where: { $0.id == draft.id }) else { return }
    if let finished {
      captures[index] = finished
    } else {
      captures.remove(at: index)
    }
  }

  func useCurrentAccount(_ id: UUID) {
    guard !busy, let index = captures.firstIndex(where: { $0.id == id && $0.account == nil }),
      let account = CaptureInbox.account()
    else { return }
    do {
      var local = captures[index]
      local.account = try CaptureInbox.archiveAccount(account)
      try Self.save(local, in: folder(id))
      captures[index] = local
      CaptureAppDelegate.scheduleHandoff()
      sync()
    } catch {
      self.problem = .operation(error.localizedDescription)
    }
  }

  var hasAccount: Bool { CaptureInbox.account() != nil }

  var hasPendingHandoff: Bool {
    if captures.contains(where: { $0.isQueued && $0.account != nil }) { return true }
    let acks = root.appendingPathComponent(".acknowledgments", isDirectory: true)
    return (try? FileManager.default.contentsOfDirectory(atPath: acks.path).isEmpty) == false
  }

  func sync(onCompletion: (() -> Void)? = nil) {
    if let onCompletion { syncCompletions.append(onCompletion) }
    guard !cloudSyncRunning else {
      cloudSyncRequested = true
      return
    }
    cloudSyncRunning = true
    cloudSyncRequested = false
    let finished = captures.filter(\.isQueued)
    worker.async { [root] in
      let (issue, requested) = Self.handoff(finished, in: root)
      Task { @MainActor [weak self] in
        guard let self else { return }
        self.cloudIssue = issue
        self.captures.removeAll {
          $0.receipt != nil && !FileManager.default.fileExists(atPath: self.folder($0.id).path)
        }
        let claimed = self.captures.filter {
          requested.contains($0.id) && $0.receipt == nil
            && ($0.capture.recording || $0.capture.hasText)
        }
        if claimed.isEmpty {
          self.finishSync()
        } else {
          self.prepareHandoff(claimed)
        }
      }
    }
  }

  private func prepareHandoff(_ captures: [LocalCapture]) {
    // Read the latest edits on the main actor before locking the capture for transfer.
    preparingHandoff = Set(captures.map(\.id))
    worker.async { [root] in
      var sealed: [UUID: LocalCapture] = [:]
      var issue: CaptureDeliveryStatus?
      for var local in captures {
        let directory = root.appendingPathComponent(local.capture.name, isDirectory: true)
        do {
          local.receipt = try CapturePackage.seal(local.capture, in: directory)
          try Self.save(local, in: directory)
          sealed[local.id] = local
        } catch {
          issue = Self.storageIssue(error) ?? issue
        }
      }
      let updates = sealed
      let failure = issue
      Task { @MainActor [weak self] in
        guard let self else { return }
        for index in self.captures.indices {
          if let local = updates[self.captures[index].id] {
            self.captures[index] = local
          }
        }
        self.preparingHandoff = []
        if let failure { self.cloudIssue = failure }
        self.finishSync(sendPrepared: !updates.isEmpty)
      }
    }
  }

  private func finishSync(sendPrepared: Bool = false) {
    cloudSyncRunning = false
    if sendPrepared {
      sync()
      return
    }
    let completions = syncCompletions
    syncCompletions.removeAll()
    for completion in completions { completion() }
    if cloudSyncRequested { sync() }
  }

  private nonisolated static func handoff(_ captures: [LocalCapture], in root: URL)
    -> (CaptureDeliveryStatus?, Set<UUID>)
  {
    var requested: Set<UUID> = []
    do {
      guard let inbox = try CaptureInbox() else {
        return (.unavailable, requested)
      }
      let files = FileManager.default
      let acks = root.appendingPathComponent(".acknowledgments", isDirectory: true)
      try files.createDirectory(at: acks, withIntermediateDirectories: true)
      var issue: CaptureDeliveryStatus?
      let receiver = (try? inbox.read("receiver.json", as: UUID?.self)) ?? nil
      for capture in captures {
        guard let account = capture.account,
          (try? inbox.checkAccount(account)) != nil
        else { continue }
        let name = capture.capture.name
        let directory = root.appendingPathComponent(name, isDirectory: true)
        do {
          guard let receipt = capture.receipt else {
            if let receiver, try inbox.read(name + ".request", as: UUID.self) == receiver {
              requested.insert(capture.id)
            } else if try !inbox.names().contains(name + ".offer") {
              try inbox.write(capture.id, to: name + ".offer")
            }
            continue
          }
          if try inbox.read(name + ".receipt", as: CaptureReceipt.self) == receipt {
            let ack = PendingAck(account: account, receipt: receipt)
            try JSONEncoder().encode(ack).write(
              to: acks.appendingPathComponent(name), options: .atomic)
            let received = acks.appendingPathComponent(".received-" + name, isDirectory: true)
            try files.moveItem(at: directory, to: received)
          } else {
            let submitted = directory.appendingPathComponent(".submitted")
            if !files.fileExists(atPath: submitted.path) {
              if try !inbox.names().contains(name + ".loof") {
                try inbox.publish(
                  directory.appendingPathComponent(CapturePackage.payload), as: name + ".loof")
              }
              // iCloud owns its upload retries after accepting the file. A
              // disappearing payload can also mean a receipt is in flight.
              try Data().write(to: submitted, options: .atomic)
            }
          }
          let status = try? inbox.root.appendingPathComponent(name + ".loof")
            .resourceValues(forKeys: [.ubiquitousItemUploadingErrorKey])
          if let error = status?.ubiquitousItemUploadingError {
            issue = storageIssue(error) ?? issue
          }
        } catch {
          issue = storageIssue(error) ?? issue
        }
      }
      for url in try files.contentsOfDirectory(
        at: acks, includingPropertiesForKeys: nil, options: .skipsHiddenFiles)
      {
        guard let id = UUID(uuidString: url.lastPathComponent),
          let data = try? Data(contentsOf: url),
          let ack = try? JSONDecoder().decode(PendingAck.self, from: data), ack.receipt.id == id,
          !files.fileExists(atPath: root.appendingPathComponent(url.lastPathComponent).path)
        else { continue }
        do {
          let name = url.lastPathComponent
          let received = acks.appendingPathComponent(".received-" + name, isDirectory: true)
          if files.fileExists(atPath: received.path) { try files.removeItem(at: received) }
          guard (try? inbox.checkAccount(ack.account)) != nil else { continue }
          if try inbox.read(name + ".receipt", as: CaptureReceipt.self) == ack.receipt {
            if try !inbox.names().contains(name + ".ack") {
              try inbox.write(ack.receipt, to: name + ".ack")
            }
          } else if try !inbox.names().contains(name + ".loof")
            && !inbox.names().contains(name + ".ack")
          {
            try files.removeItem(at: url)
          }
        } catch {
          issue = storageIssue(error) ?? issue
        }
      }
      return (issue, requested)
    } catch {
      return (storageIssue(error), requested)
    }
  }

  private nonisolated static func storageIssue(_ error: Error) -> CaptureDeliveryStatus? {
    let cocoa = error as NSError
    guard cocoa.domain == NSCocoaErrorDomain else { return nil }
    switch cocoa.code {
    case CocoaError.fileWriteOutOfSpace.rawValue: return .deviceStorage
    case CocoaError.ubiquitousFileNotUploadedDueToQuota.rawValue: return .cloudStorage
    default: return nil
    }
  }

  func deliveryStatus(for item: LocalCapture) -> CaptureDeliveryStatus {
    guard let current = CaptureInbox.account() else { return .unavailable }
    guard let account = item.account else { return .confirmAccount }
    guard CaptureInbox.matches(account, current) else { return .originalAccount }
    return cloudIssue ?? (canEdit(item) ? .waiting : .syncing)
  }

  private func folder(_ id: UUID) -> URL {
    root.appendingPathComponent(id.uuidString.lowercased(), isDirectory: true)
  }

  private nonisolated static func save(_ capture: LocalCapture, in directory: URL) throws {
    try JSONEncoder().encode(capture).write(
      to: directory.appendingPathComponent("local.json"),
      options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
  }

  private func reload() {
    let folders =
      (try? FileManager.default.contentsOfDirectory(
        at: root, includingPropertiesForKeys: nil, options: .skipsHiddenFiles)) ?? []
    captures = folders.compactMap { directory in
      guard let data = try? Data(contentsOf: directory.appendingPathComponent("local.json")) else {
        return nil
      }
      return try? JSONDecoder().decode(LocalCapture.self, from: data)
    }.sorted { $0.capture.createdAt > $1.capture.createdAt }
  }

  private func startActivity(for capture: Capture) {
    guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
    activity = try? Activity.request(
      attributes: CaptureRecordingAttributes(captureID: capture.id, startedAt: capture.createdAt),
      content: ActivityContent(state: .init(savingAt: nil), staleDate: nil),
      pushType: nil)
  }

  private func endSaveAssertion() {
    guard saveAssertion != .invalid else { return }
    UIApplication.shared.endBackgroundTask(saveAssertion)
    saveAssertion = .invalid
  }

  private func startRecorder(at url: URL, requireInput: Bool = false) throws {
    let session = AVAudioSession.sharedInstance()
    do {
      try session.setCategory(.record, mode: .default)
      try session.setActive(true)
      if requireInput, !session.isInputAvailable || session.currentRoute.inputs.isEmpty {
        throw CaptureError.invalid
      }
      let settings: [String: Any] = [
        AVFormatIDKey: kAudioFormatLinearPCM,
        AVSampleRateKey: RecordingWAV.sampleRate,
        AVNumberOfChannelsKey: 1,
        AVLinearPCMBitDepthKey: 16,
        AVLinearPCMIsBigEndianKey: false,
        AVLinearPCMIsFloatKey: false,
      ]
      let newRecorder = try AVAudioRecorder(url: url, settings: settings)
      guard newRecorder.prepareToRecord() else { throw CaptureError.invalid }
      // The recorder must be able to finish its WAV header after the phone locks.
      try FileManager.default.setAttributes(
        [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication],
        ofItemAtPath: url.path)
      newRecorder.delegate = self
      guard newRecorder.record() else { throw CaptureError.invalid }
      recorder = newRecorder
      let inputs = session.currentRoute.inputs.map { $0.portType.rawValue }.joined(separator: ",")
      Self.audioLog.info("Started recording segment; inputs=\(inputs, privacy: .public)")
    } catch {
      deactivateAudioSession()
      throw error
    }
  }

  private func deactivateAudioSession() {
    do {
      try AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    } catch {
      let issue = error as NSError
      Self.audioLog.error(
        "Audio session deactivation failed; domain=\(issue.domain, privacy: .public) code=\(issue.code, privacy: .public)"
      )
    }
  }

  private func closeRecorder(deactivate: Bool) {
    guard let stoppedRecorder = recorder else { return }
    recorder = nil
    stoppedRecorder.delegate = nil
    stoppedRecorder.stop()
    if deactivate { deactivateAudioSession() }
  }

  private func observeAudioSession(_ observationID: UUID) {
    removeAudioObservers()
    audioObservers = RecordingSessionEvent.notifications.map { name in
      NotificationCenter.default.addObserver(forName: name, object: nil, queue: nil) {
        [weak self] notification in
        guard let event = RecordingSessionEvent(notification) else { return }
        let receivedAt = Date()
        DispatchQueue.main.async {
          guard let self, self.recording?.observationID == observationID else { return }
          self.handleAudioEvent(event, at: receivedAt)
        }
      }
    }
  }

  private func removeAudioObservers() {
    for observer in audioObservers { NotificationCenter.default.removeObserver(observer) }
    audioObservers.removeAll()
  }

  private func handleAudioEvent(_ event: RecordingSessionEvent, at date: Date) {
    switch event {
    case .interruptionBegan(let reason, let wasSuspended):
      Self.audioLog.info(
        "Interruption began; reason=\(reason.map(String.init) ?? "unknown", privacy: .public) suspended=\(wasSuspended, privacy: .public)"
      )
      // A delayed suspension notification can arrive after a new recorder is already running.
      if wasSuspended, recorder?.isRecording == true { return }
      let awaitingEnd =
        !wasSuspended
        && (reason == nil || reason == AVAudioSession.InterruptionReason.default.rawValue)
      pauseRecording(at: date, awaitingEnd: awaitingEnd, deactivate: false)
    case .interruptionEnded(let options):
      Self.audioLog.info("Interruption ended; options=\(options, privacy: .public)")
      guard var current = recording else { return }
      let allowsResume = AVAudioSession.InterruptionOptions(rawValue: options).contains(
        .shouldResume)
      let shouldResume = current.interruptionEnded(allowsResume: allowsResume)
      recording = current
      if shouldResume { resume(current.id, automatic: true) }
    case .routeChanged(let reason, let previousInputs, let previousPorts):
      let inputs = AVAudioSession.sharedInstance().currentRoute.inputs
      let currentPorts = inputs.map { $0.portType.rawValue }.joined(separator: ",")
      Self.audioLog.info(
        "Route changed; reason=\(reason, privacy: .public) previous=\(previousPorts.joined(separator: ","), privacy: .public) current=\(currentPorts, privacy: .public)"
      )
      guard recorder != nil else { return }
      let inputDisconnected =
        reason == AVAudioSession.RouteChangeReason.oldDeviceUnavailable.rawValue
        && !previousInputs.isEmpty && !inputs.contains { previousInputs.contains($0.uid) }
      if inputDisconnected
        || reason == AVAudioSession.RouteChangeReason.noSuitableRouteForCategory.rawValue
      {
        pauseRecording(at: date, awaitingEnd: false, deactivate: true)
      }
    case .mediaServicesChanged(let reset):
      Self.audioLog.info("Media services event; reset=\(reset, privacy: .public)")
      pauseRecording(at: date, awaitingEnd: false, deactivate: false)
    }
  }

  private func pauseRecording(at date: Date, awaitingEnd: Bool, deactivate: Bool) {
    guard var current = recording else { return }
    let changed = current.pause(at: date, awaitingEnd: awaitingEnd)
    recording = current
    guard changed else { return }
    closeRecorder(deactivate: deactivate)
    updateActivity(for: current)
  }

  func resume(_ id: UUID, automatic: Bool = false) {
    guard !busy, var current = recording, current.id == id, current.pausedAt != nil else { return }
    current.renewObservation()
    recording = current
    observeAudioSession(current.observationID)
    var continuation: URL?
    do {
      let audio = try RecordingWAV.continuationURL(in: folder(id))
      continuation = audio
      try startRecorder(at: audio, requireInput: true)
      current.resumed(at: Date())
      recording = current
      Self.audioLog.info("Recording resumed; automatic=\(automatic, privacy: .public)")
      updateActivity(for: current)
    } catch {
      if let continuation { try? FileManager.default.removeItem(at: continuation) }
      let issue = error as NSError
      Self.audioLog.error(
        "Recording resume failed; automatic=\(automatic, privacy: .public) domain=\(issue.domain, privacy: .public) code=\(issue.code, privacy: .public)"
      )
      if !automatic {
        problem = .operation("Couldn’t resume recording. Tap Done to save the draft.")
      }
    }
  }

  private func updateActivity(for current: RecordingSession) {
    guard let activity else { return }
    let content = ActivityContent(
      state: CaptureRecordingAttributes.ContentState(
        savingAt: nil, pausedAt: current.pausedAt, timerStartedAt: current.timerStartedAt),
      staleDate: nil)
    let timestamp = Date()
    Task { await activity.update(content, timestamp: timestamp) }
  }

  nonisolated func audioRecorderDidFinishRecording(
    _ recorder: AVAudioRecorder, successfully flag: Bool
  ) {
    recorderStopped(recorder, successfully: flag, error: nil)
  }

  nonisolated func audioRecorderEncodeErrorDidOccur(_ recorder: AVAudioRecorder, error: Error?) {
    recorderStopped(recorder, successfully: false, error: error as NSError?)
  }

  private nonisolated func recorderStopped(
    _ recorder: AVAudioRecorder, successfully: Bool, error: NSError?
  ) {
    let date = Date()
    DispatchQueue.main.async { [weak self] in
      guard let self, self.recorder === recorder else { return }
      Self.audioLog.info(
        "Recorder stopped; success=\(successfully, privacy: .public) domain=\(error?.domain ?? "none", privacy: .public) code=\(error?.code ?? 0, privacy: .public)"
      )
      self.pauseRecording(at: date, awaitingEnd: false, deactivate: true)
    }
  }
}
