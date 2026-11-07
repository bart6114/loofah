import Cocoa
import Combine
import SwiftUI

final class FloatingBarManager {
  static let shared = FloatingBarManager()

  private var panel: NSPanel?
  private var appliedLayout: FloatingBarWindowLayout?
  private let model = FloatingBarViewModel()
  private let nudge = FloatingNudgeModel()
  private let settingsModel = FloatingOverlaySettingsModel.shared
  private let placement = FloatingPanelPositionController()
  private var displayChangeObserver: Any?
  private var followActiveScreenTimer: Timer?
  private var isApplyingExternalState = false
  private var frameUpdateScheduled = false
  private var cancellables = Set<AnyCancellable>()

  private init() {
    nudge.$payload
      .sink { [weak self] _ in self?.scheduleFrameUpdate() }
      .store(in: &cancellables)
    model.$isExpanded
      .removeDuplicates()
      .sink { [weak self] _ in
        guard let self, !self.isApplyingExternalState else { return }
        self.scheduleFrameUpdate()
      }
      .store(in: &cancellables)
  }

  // Every panel frame change funnels through one coalesced main-queue hop so
  // it lands after SwiftUI has committed the layout for the current model
  // state. Applying frames synchronously from event handlers paints a stale
  // frame with the pill at the left edge of the freshly widened window.
  private func scheduleFrameUpdate() {
    guard !frameUpdateScheduled else { return }
    frameUpdateScheduled = true
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      self.frameUpdateScheduled = false
      self.applyFrame()
    }
  }

  private func applyFrame() {
    guard let panel else { return }
    let layout = currentLayout
    let didResize = resize(panel, to: layout)
    if !didResize {
      position(panel, force: true, layout: layout)
    }
  }

  func show() {
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }

      if self.model.startedAt == nil {
        self.model.startedAt = Date()
      }

      if let panel = self.panel {
        self.position(panel, force: true)
        self.startFollowingActiveScreen()
        panel.orderFrontRegardless()
        return
      }

      let panel = self.createPanel()
      let hostingView = NSHostingView(
        rootView: FloatingBarView(
          model: self.model,
          nudge: self.nudge,
          settings: self.settingsModel,
          panelOrigin: { [weak self] in self?.panel?.frame.origin },
          movePanel: { [weak self] origin in
            guard let self, let panel = self.panel else { return }
            self.placement.moveByUserDrag(
              panel,
              to: origin,
              anchorOffset: self.controlAnchorOffset(for: self.currentLayout))
          }))
      hostingView.frame = NSRect(
        x: 0,
        y: 0,
        width: self.currentSize.width,
        height: self.currentSize.height)
      hostingView.autoresizingMask = [.width, .height]

      panel.contentView = hostingView
      self.position(panel, force: true)
      panel.orderFrontRegardless()
      self.panel = panel
      self.appliedLayout = self.currentLayout
      self.startFollowingActiveScreen()
    }
  }

  func hide() {
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      self.nudge.clear()
      guard let panel = self.panel else { return }
      self.stopFollowingActiveScreen()
      self.model.startedAt = nil
      panel.orderOut(nil)
      self.panel = nil
      self.appliedLayout = nil
      self.placement.resetActiveScreen()
    }
  }

  func showNudge(_ payload: FloatingNudgePayload) -> Bool {
    precondition(Thread.isMainThread)
    guard panel?.isVisible == true, model.startedAt != nil else { return false }
    nudge.show(payload)
    return true
  }

  func clearNudge(recordingId: String?) {
    precondition(Thread.isMainThread)
    nudge.clear(recordingId: recordingId)
  }

  func update(state: FloatingBarStatePayload) {
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      self.isApplyingExternalState = true
      self.model.status = state.status
      self.model.amplitude = min(max(state.amplitude, 0), 1)
      self.model.colorScheme = state.colorScheme
      self.model.title = state.title
      // Live-transcript expansion is parked for now; ignore the backend
      // toggle but keep the payload plumbing for when it returns.
      self.model.liveCaptionToggleVisible = false
      self.model.transcriptBubbles = state.transcriptBubbles
      self.settingsModel.apply(floatingBarState: state)
      self.model.isExpanded = false
      self.isApplyingExternalState = false
      self.scheduleFrameUpdate()
    }
  }

  private func createPanel() -> NSPanel {
    let panel = NSPanel(
      contentRect: NSRect(
        x: 0,
        y: 0,
        width: currentSize.width,
        height: currentSize.height),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered,
      defer: false
    )

    panel.level = .floating
    panel.isFloatingPanel = true
    panel.hidesOnDeactivate = false
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = false
    panel.sharingType = .none
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
    panel.isMovableByWindowBackground = true
    panel.minSize = currentSize
    panel.delegate = placement
    return panel
  }

  private func position(
    _ panel: NSPanel,
    force: Bool = false,
    layout targetLayout: FloatingBarWindowLayout? = nil
  ) {
    let layout = targetLayout ?? currentLayout
    let size = size(for: layout)
    placement.position(
      panel,
      force: force,
      size: size,
      anchorOffset: controlAnchorOffset(for: layout)
    ) { screen, size in
      let frame = screen.visibleFrame
      let x = frame.maxX - size.width - FloatingBarLayout.screenMargin
      let y = frame.maxY - size.height - FloatingBarLayout.screenMargin
      return NSPoint(x: x, y: y)
    }
  }

  private func resize(
    _ panel: NSPanel,
    to targetLayout: FloatingBarWindowLayout? = nil
  ) -> Bool {
    let nextLayout = targetLayout ?? currentLayout
    let size = size(for: nextLayout)
    let previousSize = panel.frame.size
    panel.minSize = size
    guard previousSize != size else { return false }

    let previousLayout = appliedLayout ?? nextLayout
    let previousAnchorOffset = controlAnchorOffset(for: previousLayout)
    let nextAnchorOffset = controlAnchorOffset(for: nextLayout)
    let anchor = placement.anchorPoint(for: panel, offset: previousAnchorOffset)
    let frame = NSRect(
      x: anchor.x - nextAnchorOffset.x,
      y: anchor.y - nextAnchorOffset.y,
      width: size.width,
      height: size.height)
    placement.setFrame(
      panel,
      to: frame,
      display: true,
      animate: false,
      anchorOffset: nextAnchorOffset)
    panel.contentView?.frame = NSRect(origin: .zero, size: size)
    appliedLayout = nextLayout
    position(panel, force: true, layout: nextLayout)
    return true
  }

  private var currentSize: NSSize {
    size(for: currentLayout)
  }

  private var currentLayout: FloatingBarWindowLayout {
    layout(isExpanded: model.isExpanded)
  }

  private func layout(isExpanded: Bool) -> FloatingBarWindowLayout {
    FloatingBarWindowLayout(
      isExpanded: isExpanded,
      showsExpand: model.liveCaptionToggleVisible,
      nudgeHeight: nudge.payload.map { FloatingBarLayout.nudgeHeight(text: $0.text) } ?? 0
    )
  }

  private func size(for layout: FloatingBarWindowLayout) -> NSSize {
    let base = FloatingBarLayout.containerSize(
      isExpanded: layout.isExpanded,
      showsExpand: layout.showsExpand
    )
    guard layout.nudgeHeight > 0 else { return base }
    return NSSize(
      width: max(base.width, FloatingBarLayout.nudgeWidth + FloatingBarLayout.inset * 2),
      height: base.height + layout.nudgeHeight + FloatingBarLayout.nudgeGap)
  }

  // Anchor at the pill's top-right corner so layout changes keep the resting
  // pill in place.
  private func controlAnchorOffset(for layout: FloatingBarWindowLayout) -> NSPoint {
    let nudgeHeight = layout.nudgeHeight > 0 ? layout.nudgeHeight + FloatingBarLayout.nudgeGap : 0
    if layout.isExpanded {
      return NSPoint(
        x: FloatingBarLayout.inset + FloatingBarLayout.expandedWidth,
        y: FloatingBarLayout.inset + FloatingBarLayout.expandedHeight + nudgeHeight
      )
    }

    return NSPoint(
      x: size(for: layout).width - FloatingBarLayout.inset,
      y: FloatingBarLayout.inset + FloatingBarLayout.compactHeight + nudgeHeight
    )
  }

  private func startFollowingActiveScreen() {
    guard followActiveScreenTimer == nil else { return }

    let timer = Timer(timeInterval: 0.25, repeats: true) { [weak self] _ in
      self?.scheduleFrameUpdate()
    }
    RunLoop.main.add(timer, forMode: .common)
    followActiveScreenTimer = timer

    displayChangeObserver = NotificationCenter.default.addObserver(
      forName: NSApplication.didChangeScreenParametersNotification,
      object: nil,
      queue: .main
    ) { [weak self] _ in
      self?.scheduleFrameUpdate()
    }
  }

  private func stopFollowingActiveScreen() {
    followActiveScreenTimer?.invalidate()
    followActiveScreenTimer = nil

    if let displayChangeObserver {
      NotificationCenter.default.removeObserver(displayChangeObserver)
      self.displayChangeObserver = nil
    }
  }

}

private struct FloatingBarWindowLayout {
  let isExpanded: Bool
  let showsExpand: Bool
  var nudgeHeight: CGFloat = 0
}
