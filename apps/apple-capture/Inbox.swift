import Foundation

struct CaptureInbox {
  static let container = "iCloud.io.loofah.capture"
  static let directory = "CaptureInbox-v1"

  let root: URL
  private let identity: NSObject

  static func account() -> NSObject? {
    FileManager.default.ubiquityIdentityToken as? NSObject
  }

  static func archiveAccount(_ identity: NSObject) throws -> Data {
    try NSKeyedArchiver.archivedData(withRootObject: identity, requiringSecureCoding: false)
  }

  static func matches(_ archived: Data, _ identity: NSObject) -> Bool {
    guard let decoder = try? NSKeyedUnarchiver(forReadingFrom: archived) else { return false }
    // Apple's opaque identity token promises NSCoding, including across launches.
    decoder.requiresSecureCoding = false
    decoder.decodingFailurePolicy = .setErrorAndReturn
    defer { decoder.finishDecoding() }
    return identity.isEqual(decoder.decodeObject(forKey: NSKeyedArchiveRootObjectKey))
  }

  init?() throws {
    guard let identity = Self.account(),
      let container = FileManager.default.url(forUbiquityContainerIdentifier: Self.container)
    else { return nil }
    self.identity = identity
    root = container.appendingPathComponent(Self.directory, isDirectory: true)
    try checkAccount()
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
  }

  func checkAccount(_ archived: Data? = nil) throws {
    guard identity.isEqual(Self.account()), archived.map({ Self.matches($0, identity) }) != false
    else {
      throw CaptureError.accountChanged
    }
  }

  func names() throws -> [String] {
    try checkAccount()
    return try FileManager.default.contentsOfDirectory(atPath: root.path).map {
      if $0.hasPrefix("."), $0.hasSuffix(".icloud") { return String($0.dropFirst().dropLast(7)) }
      return $0
    }
  }

  func read<T>(_ name: String, allowConflicts: Bool = false, _ body: (URL) throws -> T) throws -> T?
  {
    try checkAccount()
    let url = root.appendingPathComponent(name)
    guard try names().contains(name) else { return nil }
    let values = try? url.resourceValues(forKeys: [
      .ubiquitousItemDownloadingStatusKey, .ubiquitousItemHasUnresolvedConflictsKey,
    ])
    guard values?.ubiquitousItemDownloadingStatus == .current,
      allowConflicts || values?.ubiquitousItemHasUnresolvedConflicts != true
    else {
      try? FileManager.default.startDownloadingUbiquitousItem(at: url)
      throw CaptureError.waiting
    }
    var error: NSError?
    var result: Result<T, Error>?
    NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &error) { coordinated in
      result = Result {
        try checkAccount()
        let latest = try coordinated.resourceValues(forKeys: [
          .ubiquitousItemDownloadingStatusKey, .ubiquitousItemHasUnresolvedConflictsKey,
        ])
        guard latest.ubiquitousItemDownloadingStatus == .current,
          allowConflicts || latest.ubiquitousItemHasUnresolvedConflicts != true
        else { throw CaptureError.waiting }
        return try body(coordinated)
      }
    }
    if let error { throw error }
    guard let result else { throw CaptureError.waiting }
    return try result.get()
  }

  func read<T: Decodable>(_ name: String, as: T.Type, allowConflicts: Bool = false) throws -> T? {
    try read(name, allowConflicts: allowConflicts) {
      try JSONDecoder().decode(T.self, from: Data(contentsOf: $0))
    }
  }

  func write<T: Encodable>(_ value: T, to name: String) throws {
    let data = try JSONEncoder().encode(value)
    try coordinateWrite(name, options: .forReplacing) { try data.write(to: $0, options: .atomic) }
  }

  func designate(_ receiver: UUID?, expected: UUID?) throws {
    // The coordinated comparison keeps a stale settings view from replacing a newer choice.
    let data = try JSONEncoder().encode(receiver)
    try coordinateWrite("receiver.json", options: .forReplacing) { url in
      if FileManager.default.fileExists(atPath: url.path) {
        let values = try url.resourceValues(forKeys: [.ubiquitousItemDownloadingStatusKey])
        guard values.ubiquitousItemDownloadingStatus == .current,
          try JSONDecoder().decode(UUID?.self, from: Data(contentsOf: url)) == expected
        else {
          throw CaptureError.waiting
        }
      } else if expected != nil || (try? names().contains("receiver.json")) == true {
        throw CaptureError.waiting
      }
      try data.write(to: url, options: .atomic)
      for version in NSFileVersion.unresolvedConflictVersionsOfItem(at: url) ?? [] {
        version.isResolved = true
      }
    }
  }

  func publish(_ source: URL, as name: String) throws {
    let staging = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: staging) }
    try FileManager.default.copyItem(at: source, to: staging)
    try checkAccount()
    // setUbiquitous coordinates its own move.
    try FileManager.default.setUbiquitous(
      true, itemAt: staging, destinationURL: root.appendingPathComponent(name))
  }

  func remove(_ name: String) throws {
    guard try names().contains(name) else { return }
    try coordinateWrite(name, options: .forDeleting) {
      do { try FileManager.default.removeItem(at: $0) } catch let error as CocoaError
        where error.code == .fileNoSuchFile
      {}
    }
  }

  private func coordinateWrite(
    _ name: String, options: NSFileCoordinator.WritingOptions, _ body: (URL) throws -> Void
  ) throws {
    try checkAccount()
    var error: NSError?
    var result: Result<Void, Error>?
    NSFileCoordinator().coordinate(
      writingItemAt: root.appendingPathComponent(name), options: options, error: &error
    ) { url in
      result = Result {
        try checkAccount()
        try body(url)
      }
    }
    if let error { throw error }
    guard let result else { throw CaptureError.waiting }
    try result.get()
  }
}

final class CaptureInboxObserver: NSObject {
  private let query = NSMetadataQuery()
  private let changed: () -> Void

  init(changed: @escaping () -> Void) {
    self.changed = changed
    super.init()
    query.searchScopes = [NSMetadataQueryUbiquitousDataScope]
    query.predicate = NSCompoundPredicate(
      orPredicateWithSubpredicates: [
        NSPredicate(format: "%K == %@", NSMetadataItemFSNameKey, "receiver.json")
      ]
        + [".loof", ".receipt", ".ack", ".offer", ".request"].map {
          NSPredicate(format: "%K ENDSWITH %@", NSMetadataItemFSNameKey, $0)
        })
    let center = NotificationCenter.default
    center.addObserver(
      self, selector: #selector(notify), name: .NSMetadataQueryDidFinishGathering, object: query)
    center.addObserver(
      self, selector: #selector(notify), name: .NSMetadataQueryDidUpdate, object: query)
    center.addObserver(
      self, selector: #selector(identityChanged), name: .NSUbiquityIdentityDidChange, object: nil)
    query.start()
  }

  @objc private func notify() { changed() }

  @objc private func identityChanged() {
    DispatchQueue.main.async { [weak self] in
      self?.query.stop()
      self?.query.start()
      self?.changed()
    }
  }

  deinit {
    query.stop()
    NotificationCenter.default.removeObserver(self)
  }
}
