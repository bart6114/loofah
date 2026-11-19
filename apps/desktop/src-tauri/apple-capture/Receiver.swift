import Darwin
import Foundation
import Security

private var observer: CaptureInboxObserver?

@_cdecl("loofah_capture_is_entitled")
func loofahCaptureIsEntitled() -> Bool {
  guard let task = SecTaskCreateFromSelf(nil),
    let containers = SecTaskCopyValueForEntitlement(
      task, "com.apple.developer.ubiquity-container-identifiers" as CFString, nil
    ) as? [String]
  else { return false }
  return containers.contains(CaptureInbox.container)
}

@_cdecl("loofah_capture_observe")
func loofahCaptureObserve(_ callback: @escaping @convention(c) () -> Void) {
  DispatchQueue.main.async {
    observer = CaptureInboxObserver(changed: callback)
  }
}

private struct Request: Decodable {
  let operation: String
  let vault: String
  let id: UUID?
  let destination: String?
  let expectedReceiver: UUID?
  let receipt: CaptureReceipt?
  let account: Data?
}

private struct Reply: Encodable {
  var available = true
  var enabled = false
  var receiver: UUID?
  var captures: [UUID] = []
  var receipt: CaptureReceipt?
  var account: Data?
  var error: String?
}

@_cdecl("loofah_capture_request")
func loofahCaptureRequest(_ raw: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? {
  autoreleasepool {
    let reply: Reply
    do {
      let request = try JSONDecoder().decode(Request.self, from: Data(String(cString: raw).utf8))
      reply = try handle(request)
    } catch {
      reply = Reply(error: error.localizedDescription)
    }
    guard let data = try? JSONEncoder().encode(reply) else { return nil }
    return strdup(String(decoding: data, as: UTF8.self))
  }
}

@_cdecl("loofah_capture_free")
func loofahCaptureFree(_ raw: UnsafeMutablePointer<CChar>) { free(raw) }

private func handle(_ request: Request) throws -> Reply {
  guard let inbox = try CaptureInbox() else {
    if request.operation == "status" || request.operation == "list" {
      return Reply(available: false)
    }
    throw CaptureError.waiting
  }
  try inbox.checkAccount(request.account)
  let defaults = UserDefaults.standard
  let settingsKey = "CaptureReceiverIDs"
  var receivers = defaults.dictionary(forKey: settingsKey) as? [String: String] ?? [:]
  let local = receivers[request.vault].flatMap(UUID.init(uuidString:))
  let resolveConflicts = ["status", "enable", "disable", "finish"].contains(request.operation)
  let receiver =
    try inbox.read("receiver.json", as: UUID?.self, allowConflicts: resolveConflicts) ?? nil
  let receiverURL = inbox.root.appendingPathComponent("receiver.json")
  let unresolved = NSFileVersion.unresolvedConflictVersionsOfItem(at: receiverURL)?.isEmpty == false
  let enabled = !unresolved && local != nil && local == receiver
  let account = try CaptureInbox.account().map(CaptureInbox.archiveAccount)
  try inbox.checkAccount(account)
  var reply = Reply(enabled: enabled, receiver: receiver, account: account)

  switch request.operation {
  case "enable":
    let receiverID = local ?? UUID()
    receivers[request.vault] = receiverID.uuidString
    defaults.set(receivers, forKey: settingsKey)
    try inbox.designate(receiverID, expected: request.expectedReceiver)
    reply = Reply(enabled: true, receiver: receiverID)
  case "disable":
    guard enabled, receiver == request.expectedReceiver else { throw CaptureError.waiting }
    try inbox.designate(nil, expected: receiver)
    reply = Reply()
  case "list":
    guard enabled else { break }
    let names = try inbox.names()
    let extensions = Set(["loof", "receipt", "ack"])
    reply.captures = Array(
      Set(
        names.compactMap { name -> UUID? in
          let parts = name.split(separator: ".")
          guard parts.count == 2, extensions.contains(String(parts[1])) else { return nil }
          return UUID(uuidString: String(parts[0]))
        })
    ).sorted { $0.uuidString < $1.uuidString }
  case "stage":
    guard enabled, let id = request.id, let destination = request.destination else { break }
    let name = id.uuidString.lowercased()
    if let receipt = try inbox.read(name + ".receipt", as: CaptureReceipt.self) {
      guard receipt.id == id else { throw CaptureError.invalid }
      try clean(inbox, name: name, receipt: receipt)
      break
    }
    if let ack = try inbox.read(name + ".ack", as: CaptureReceipt.self) {
      guard ack.id == id else { throw CaptureError.invalid }
      try clean(inbox, name: name, receipt: ack)
      break
    }
    reply.receipt = try inbox.read(name + ".loof") { url in
      try CapturePackage.unpack(
        url, into: URL(fileURLWithPath: destination, isDirectory: true), id: id)
    }
  case "finish":
    guard let receipt = request.receipt, request.account != nil else { throw CaptureError.invalid }
    let name = receipt.id.uuidString.lowercased()
    if let existing = try inbox.read(name + ".receipt", as: CaptureReceipt.self) {
      guard existing == receipt else { throw CaptureError.invalid }
    } else if let ack = try inbox.read(name + ".ack", as: CaptureReceipt.self) {
      guard ack == receipt else { throw CaptureError.invalid }
    } else {
      try inbox.write(receipt, to: name + ".receipt")
    }
    try clean(inbox, name: name, receipt: receipt)
  case "status":
    break
  default:
    throw CaptureError.invalid
  }
  return reply
}

private func clean(_ inbox: CaptureInbox, name: String, receipt: CaptureReceipt) throws {
  try inbox.remove(name + ".loof")
  guard try inbox.read(name + ".ack", as: CaptureReceipt.self) == receipt else { return }
  try inbox.remove(name + ".receipt")
  try inbox.remove(name + ".ack")
}
