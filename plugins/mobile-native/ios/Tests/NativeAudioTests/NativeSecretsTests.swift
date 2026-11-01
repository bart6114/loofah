import Foundation
import Security
import XCTest

@testable import tauri_plugin_mobile_native

final class NativeSecretsTests: XCTestCase {
  func testProviderKeysHaveSeparateDeviceOnlyNamespaces() throws {
    let openrouter = try NativeSecrets.query(providerId: "openrouter")
    let openai = try NativeSecrets.query(providerId: "openai")
    XCTAssertEqual(openrouter[kSecAttrAccount as String] as? String, "llm:openrouter")
    XCTAssertEqual(openai[kSecAttrAccount as String] as? String, "llm:openai")
    XCTAssertEqual(
      openai[kSecAttrService as String] as? String, "io.loofah.notes.ai-provider-api-keys")
    XCTAssertEqual(openai[kSecAttrSynchronizable as String] as? Bool, false)
  }

  func testInvalidProviderCannotAccessKeychain() {
    for provider in [
      "", "llm:openrouter", "../openrouter", "OpenAI", "a b", "🔑",
      String(repeating: "a", count: 65),
    ] {
      XCTAssertThrowsError(
        try NativeSecrets.read(
          providerId: provider,
          copy: { _ in
            XCTFail("Invalid provider reached Keychain")
            return nil
          }))
    }
  }

  func testLegacyOpenRouterIsCopiedWithoutDeletingOriginal() throws {
    let key = Data("legacy-secret".utf8)
    var reads: [String] = []
    var saved: Data?
    let result = try NativeSecrets.read(
      providerId: "openrouter",
      copy: { query in
        let service = query[kSecAttrService as String] as! String
        reads.append(service)
        return service == "io.loofah.notes.openrouter" ? key : nil
      },
      save: { query, data in
        XCTAssertEqual(query[kSecAttrAccount as String] as? String, "llm:openrouter")
        saved = data
      })
    XCTAssertEqual(result, "legacy-secret")
    XCTAssertEqual(saved, key)
    XCTAssertEqual(reads, ["io.loofah.notes.ai-provider-api-keys", "io.loofah.notes.openrouter"])
  }

  func testExistingKeyAndOtherProvidersNeverReadLegacyEntry() throws {
    for provider in ["openrouter", "openai"] {
      var reads = 0
      let result = try NativeSecrets.read(
        providerId: provider,
        copy: { query in
          reads += 1
          XCTAssertEqual(
            query[kSecAttrService as String] as? String, "io.loofah.notes.ai-provider-api-keys")
          return provider == "openrouter" ? Data("current-secret".utf8) : nil
        }, save: { _, _ in XCTFail("Unexpected migration") })
      XCTAssertEqual(reads, 1)
      XCTAssertEqual(result, provider == "openrouter" ? "current-secret" : nil)
    }
  }

  func testClearingOpenRouterCannotResurrectLegacyKey() throws {
    var marker: Data?
    try NativeSecrets.set(
      providerId: "openrouter", value: nil,
      save: { _, data in
        marker = data
      }, remove: { _ in XCTFail("Clearing must preserve a migration marker") })
    XCTAssertEqual(marker, Data())
    let result = try NativeSecrets.read(
      providerId: "openrouter",
      copy: { query in
        XCTAssertEqual(
          query[kSecAttrService as String] as? String, "io.loofah.notes.ai-provider-api-keys")
        return marker
      }, save: { _, _ in XCTFail("Cleared key must not migrate") })
    XCTAssertNil(result)
  }

  func testOtherProviderClearingRemovesOnlyItsKey() throws {
    var removed = false
    try NativeSecrets.set(
      providerId: "anthropic", value: "  ",
      save: { _, _ in
        XCTFail("Empty key must not be stored")
      },
      remove: { query in
        removed = true
        XCTAssertEqual(query[kSecAttrAccount as String] as? String, "llm:anthropic")
      })
    XCTAssertTrue(removed)
  }
}
