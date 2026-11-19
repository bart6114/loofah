import XCTest

final class CaptureUITests: XCTestCase {
  override func setUp() {
    continueAfterFailure = false
  }

  func testOfflineDraftAndFinishedNoteSurviveRestart() {
    let app = XCUIApplication()
    app.launch()
    XCTAssertTrue(app.buttons["Record"].waitForExistence(timeout: 5))
    XCTAssertTrue(app.buttons["New note"].exists)
    XCTAssertTrue(app.staticTexts["Waiting for sync"].exists)
    screenshot(app, name: "01-inbox")

    app.buttons["New note"].tap()
    let title = app.textFields["Title"]
    XCTAssertTrue(title.waitForExistence(timeout: 5))
    title.tap()
    title.typeText("Studio check-in")
    let editor = app.textViews["Notes"]
    XCTAssertTrue(editor.waitForExistence(timeout: 5))
    editor.tap()
    editor.typeText("Bring the printed sketches to Thursday’s review.")
    screenshot(app, name: "02-note")

    app.terminate()
    app.launch()
    let capture = app.buttons["Studio check-in"].firstMatch
    XCTAssertTrue(capture.waitForExistence(timeout: 5))
    XCTAssertTrue((capture.value as? String)?.hasPrefix("Note, Draft, ") == true)
    capture.tap()
    XCTAssertTrue(editor.exists)
    app.buttons["Done"].tap()

    XCTAssertTrue(capture.waitForExistence(timeout: 5))
    XCTAssertFalse((capture.value as? String)?.contains("Draft") == true)
    app.terminate()
    app.launch()
    XCTAssertTrue(capture.waitForExistence(timeout: 5))
    capture.tap()
    XCTAssertEqual(title.value as? String, "Studio check-in")
    XCTAssertTrue((editor.value as? String)?.contains("printed sketches") == true)
    title.tap()
    title.typeText(" follow-up")
    editor.tap()
    editor.typeText(" Bring pens too.")
    let revisedTitle = title.value as? String ?? ""
    XCTAssertTrue(revisedTitle.contains("follow-up"))
    app.buttons["Done"].tap()
    app.terminate()
    app.launch()
    let revisedCapture = app.buttons[revisedTitle].firstMatch
    XCTAssertTrue(revisedCapture.waitForExistence(timeout: 5))
    revisedCapture.tap()
    XCTAssertTrue((editor.value as? String)?.contains("Bring pens too.") == true)
    screenshot(app, name: "04-finished")
  }

  func testEmptyDraftIsDiscardedWhenLeaving() {
    let app = XCUIApplication()
    app.launch()
    let newNote = app.buttons["New note"]
    newNote.tap()
    let back = app.navigationBars.buttons.matching(NSPredicate(format: "label != %@", "Done"))
      .firstMatch
    XCTAssertTrue(back.waitForExistence(timeout: 5))
    back.tap()
    XCTAssertTrue(newNote.waitForExistence(timeout: 5))
    XCTAssertFalse(app.buttons["Note"].exists)
  }

  func testMacHandoffHelp() {
    let app = XCUIApplication()
    app.launch()
    app.buttons["Mac handoff"].tap()
    XCTAssertTrue(app.navigationBars["Mac handoff"].waitForExistence(timeout: 5))
    XCTAssertTrue(
      app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "iCloud Drive"))
        .firstMatch.exists)
    XCTAssertTrue(app.descendants(matching: .any)["Privacy policy"].firstMatch.exists)
    app.buttons["Done"].tap()
    XCTAssertTrue(app.buttons["New note"].isHittable)
  }

  func testMicrophoneDenialOffersSettings() {
    let app = XCUIApplication()
    app.launch()
    app.resetAuthorizationStatus(for: .microphone)
    app.launch()
    defer { app.resetAuthorizationStatus(for: .microphone) }
    app.buttons["Record"].tap()

    let deny = XCUIApplication(bundleIdentifier: "com.apple.springboard").alerts.buttons
      .matching(NSPredicate(format: "label BEGINSWITH %@", "Don")).firstMatch
    XCTAssertTrue(deny.waitForExistence(timeout: 5))
    deny.tap()
    let alert = app.alerts["Microphone access"]
    XCTAssertTrue(alert.buttons["Open Settings"].waitForExistence(timeout: 5))
    alert.buttons["Cancel"].tap()
    XCTAssertTrue(app.buttons["New note"].isHittable)
  }

  func testAudioOnlyCaptureShowsRecording() {
    let app = XCUIApplication()
    app.launch()
    startRecording(in: app)
    app.buttons["Stop"].tap()

    let capture = app.buttons["Recording"].firstMatch
    XCTAssertTrue(capture.waitForExistence(timeout: 10))
    XCTAssertTrue((capture.value as? String)?.hasPrefix("Recording, ") == true)
    XCTAssertFalse((capture.value as? String)?.contains(", Note,") == true)
  }

  func testRecordingFinishesOnRestart() {
    let app = XCUIApplication()
    app.launch()
    startRecording(in: app)
    let title = "Recovered recording \(UUID().uuidString.prefix(8))"
    let editor = app.textViews["Notes"]
    editor.tap()
    editor.typeText(title)

    app.terminate()
    app.launch()
    let capture = app.buttons[title]
    XCTAssertTrue(capture.waitForExistence(timeout: 10))
    capture.tap()
    XCTAssertTrue(app.staticTexts["Audio recording"].waitForExistence(timeout: 10))
    XCTAssertTrue((editor.value as? String)?.contains(title) == true)
  }

  func testRecordingContinuesOnHomeScreen() {
    let app = XCUIApplication()
    app.launch()
    startRecording(in: app)
    let editor = app.textViews["Notes"]
    editor.tap()
    editor.typeText("Background recording check")

    XCUIDevice.shared.press(.home)
    XCTAssertTrue(app.wait(for: .runningBackground, timeout: 5))
    let home = XCUIApplication(bundleIdentifier: "com.apple.springboard")
    home.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.036)).press(forDuration: 1)
    let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
    attachment.name = "recording-live-activity"
    attachment.lifetime = .keepAlways
    add(attachment)

    let stopActivity = home.buttons["Stop and save recording"].firstMatch
    XCTAssertTrue(stopActivity.waitForExistence(timeout: 5), home.debugDescription)
    stopActivity.tap()
    XCTAssertNotEqual(app.state, .runningForeground)

    app.activate()
    XCTAssertTrue(app.staticTexts["Audio recording"].waitForExistence(timeout: 10))
    XCTAssertTrue((editor.value as? String)?.contains("Background recording check") == true)
  }

  func testRecordingAndNotesFinishTogether() {
    let app = XCUIApplication()
    app.launch()
    startRecording(in: app)
    let title = app.textFields["Title"]
    title.tap()
    title.typeText("Design review")
    let editor = app.textViews["Notes"]
    editor.tap()
    editor.typeText("Discuss the new layout and check the print samples.")
    XCTAssertTrue(app.keyboards.firstMatch.exists)
    XCTAssertTrue(app.buttons["Stop"].isHittable)
    screenshot(app, name: "03-recording")
    app.buttons["Stop"].tap()
    let capture = app.buttons["Design review"].firstMatch
    XCTAssertTrue(capture.waitForExistence(timeout: 10))
    XCTAssertTrue((capture.value as? String)?.hasPrefix("Recording, Note, ") == true)
    XCTAssertFalse((capture.value as? String)?.contains("Draft") == true)
    capture.tap()
    XCTAssertEqual(title.value as? String, "Design review")
    XCTAssertTrue((editor.value as? String)?.contains("print samples") == true)
  }

  private func startRecording(in app: XCUIApplication) {
    app.buttons["Record"].tap()
    let permission = XCUIApplication(bundleIdentifier: "com.apple.springboard").alerts.buttons[
      "Allow"]
    if permission.waitForExistence(timeout: 3) { permission.tap() }
    XCTAssertTrue(app.buttons["Stop"].waitForExistence(timeout: 10), app.debugDescription)
  }

  private func screenshot(_ app: XCUIApplication, name: String) {
    let attachment = XCTAttachment(screenshot: app.screenshot())
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
  }
}
