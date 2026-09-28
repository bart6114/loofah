import Foundation
import Security
import Tauri

struct ApiKeyArgs: Decodable {
  let providerId: String
  let value: String?
}

struct ReadApiKeyArgs: Decodable { let providerId: String }

enum NativeSecrets {
  static func setApiKey(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ApiKeyArgs.self)
    try set(providerId: args.providerId, value: args.value)
    invoke.resolve()
  }

  static func readApiKey(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ReadApiKeyArgs.self)
    invoke.resolve(try read(providerId: args.providerId))
  }

  static func query(providerId: String) throws -> [String: Any] {
    guard !providerId.isEmpty, providerId.utf8.count <= 64,
      providerId.utf8.allSatisfy({
        (97...122).contains($0) || (48...57).contains($0) || $0 == 95 || $0 == 45
      })
    else { throw NativeFailure(message: "Invalid AI provider identifier.") }
    return [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "io.loofah.mobile.ai-provider-api-keys",
      kSecAttrAccount as String: "llm:\(providerId)",
      kSecAttrSynchronizable as String: false,
    ]
  }

  static func read(
    providerId: String,
    copy: ([String: Any]) throws -> Data? = copyData,
    save: ([String: Any], Data) throws -> Void = saveData
  ) throws -> String? {
    let destination = try query(providerId: providerId)
    if let data = try copy(destination) { return try decode(data) }
    guard providerId == "openrouter" else { return nil }
    let legacy: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "io.loofah.mobile.openrouter",
      kSecAttrAccount as String: "api-key",
      kSecAttrSynchronizable as String: false,
    ]
    guard let data = try copy(legacy), let key = try decode(data) else { return nil }
    try save(destination, data)
    return key
  }

  static func set(
    providerId: String, value: String?,
    save: ([String: Any], Data) throws -> Void = saveData,
    remove: ([String: Any]) throws -> Void = removeData
  ) throws {
    let destination = try query(providerId: providerId)
    let value = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard value.utf8.count <= 16_384 else {
      throw NativeFailure(message: "The API key is too long.")
    }
    if !value.isEmpty || providerId == "openrouter" {
      // An empty OpenRouter item prevents a cleared key from being restored from the legacy item.
      try save(destination, Data(value.utf8))
    } else {
      try remove(destination)
    }
  }

  private static func decode(_ data: Data) throws -> String? {
    guard let value = String(data: data, encoding: .utf8) else {
      throw NativeFailure(message: "Could not read the API key from Keychain.")
    }
    return value.isEmpty ? nil : value
  }

  private static func saveData(_ query: [String: Any], _ data: Data) throws {
    let attributes: [String: Any] = [
      kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    let updated = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    if updated == errSecItemNotFound {
      var item = query
      item.merge(attributes) { _, new in new }
      guard SecItemAdd(item as CFDictionary, nil) == errSecSuccess else {
        throw NativeFailure(message: "Could not save the API key in Keychain.")
      }
    } else if updated != errSecSuccess {
      throw NativeFailure(message: "Could not update the API key in Keychain.")
    }
  }

  private static func removeData(_ query: [String: Any]) throws {
    let status = SecItemDelete(query as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw NativeFailure(message: "Could not remove the API key from Keychain.")
    }
  }

  private static func copyData(_ query: [String: Any]) throws -> Data? {
    var query = query
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &result)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess, let data = result as? Data else {
      throw NativeFailure(message: "Could not read the API key from Keychain.")
    }
    return data
  }
}
