import AppKit
import SwiftUI

enum FloatingBarLayout {
  static let nudgeWidth: CGFloat = 320
  static func nudgeHeight(text: String) -> CGFloat {
    let bounds = (text as NSString).boundingRect(
      with: NSSize(width: nudgeWidth - 24, height: .greatestFiniteMagnitude),
      options: [.usesLineFragmentOrigin, .usesFontLeading],
      attributes: [.font: NSFont.systemFont(ofSize: 13)])
    return max(90, ceil(bounds.height) + 64)
  }
  static let nudgeGap: CGFloat = 6
  static let inset: CGFloat = 4
  static let screenMargin: CGFloat = 8
  static let compactHeight: CGFloat = 38
  static let compactRestWidth: CGFloat = 112
  static let compactControlSize: CGFloat = 26
  static let compactControlGap: CGFloat = 2
  static let compactContentSpacing: CGFloat = 8
  static let compactLeadingPadding: CGFloat = 12
  static let compactTrailingPadding: CGFloat = 13
  static let compactTimerWidth: CGFloat = 48
  static let compactTraceWidth: CGFloat = 110
  static let compactTraceHeight: CGFloat = 20
  static let compactHorizontalPadding: CGFloat = 4
  static let compactCornerControlFactor: CGFloat = 0.55228475
  static let expandedWidth: CGFloat = 360
  static let expandedHeight: CGFloat = 430
  static let expandedCornerRadius: CGFloat = 21
  static let expandedPadding: CGFloat = 12
  static let hoverHandleGap: CGFloat = 2
  static let hoverHandleTopPadding: CGFloat = 7
  static let hoverHandleHeight: CGFloat = 12
  static let hoverHandleReservedHeight: CGFloat =
    hoverHandleTopPadding + hoverHandleHeight + hoverHandleGap
  static let hoverHandleDotSize: CGFloat = 1.6
  static let hoverHandleDotSpacing: CGFloat = 7
  static let hoverHandleHorizontalPadding: CGFloat = 17
  static let dragClickThreshold: CGFloat = 4

  static func compactControlsWidth(showsExpand: Bool) -> CGFloat {
    if showsExpand {
      return compactControlSize * 2 + compactControlGap
    }

    return compactControlSize
  }

  static func compactPillWidth(hovered: Bool, showsExpand: Bool) -> CGFloat {
    guard hovered else { return compactRestWidth }

    return compactLeadingPadding
      + compactControlsWidth(showsExpand: showsExpand)
      + compactContentSpacing + compactTimerWidth
      + compactContentSpacing + compactTraceWidth
      + compactTrailingPadding
  }

  // The compact window is always sized for the fully hovered pill; hover only
  // animates content inside it. Resizing the window on hover repaints one
  // frame with stale content at the new origin, which reads as a jump.
  static func containerSize(isExpanded: Bool, showsExpand: Bool) -> NSSize {
    if isExpanded {
      return NSSize(
        width: expandedWidth + inset * 2,
        height: expandedHeight + hoverHandleReservedHeight + inset * 2)
    }

    return NSSize(
      width: compactPillWidth(hovered: true, showsExpand: showsExpand) + inset * 2,
      height: compactHeight + inset * 2)
  }
}

struct FloatingBarView: View {
  @ObservedObject var model: FloatingBarViewModel
  @ObservedObject var nudge: FloatingNudgeModel
  @ObservedObject var settings: FloatingOverlaySettingsModel
  let panelOrigin: () -> NSPoint?
  let movePanel: (NSPoint) -> Void
  @State private var isBarHovered = false
  @State private var shouldAutoScrollTranscript = true
  @State private var suppressNextClick = false
  @State private var dragStart: FloatingBarDragStart?
  private let transcriptBottomAnchorId = "floating-transcript-bottom-anchor"

  var body: some View {
    VStack(alignment: .trailing, spacing: FloatingBarLayout.nudgeGap) {
      if model.isExpanded {
        expandedPanel
      } else {
        compactPill
      }
      if let payload = nudge.payload {
        nudgeCard(payload)
      }
    }
    .padding(FloatingBarLayout.inset)
    // Fill whatever size the panel currently has and pin content to the
    // trailing edge; hover and drag are scoped to the visible surfaces so the
    // window's empty leading strip stays inert.
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
  }

  private func nudgeCard(_ payload: FloatingNudgePayload) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack {
        Text("Nudge")
          .font(.system(size: 11, weight: .semibold))
        Spacer()
        Button {
          nudge.clear()
        } label: {
          Image(systemName: "xmark")
            .frame(width: 24, height: 24)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Dismiss nudge")
      }
      Text(payload.text)
        .font(.system(size: 13))
        .fixedSize(horizontal: false, vertical: true)
        .textSelection(.disabled)
      Spacer(minLength: 0)
    }
    .foregroundStyle(primaryContentColor)
    .padding(12)
    .frame(
      width: FloatingBarLayout.nudgeWidth,
      height: FloatingBarLayout.nudgeHeight(text: payload.text), alignment: .topLeading
    )
    .background {
      ZStack {
        VisualEffectBlur(colorScheme: model.colorScheme, cornerRadius: 16)
        RoundedRectangle(cornerRadius: 16).fill(surfaceTintColor)
      }
    }
    .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(outerStrokeColor, lineWidth: 0.5))
    .clipShape(RoundedRectangle(cornerRadius: 16))
    .onHover { nudge.setHovering($0) }
    .accessibilityElement(children: .contain)
  }

  private var compactPill: some View {
    let radius = FloatingBarLayout.compactHeight / 2
    let pillShape = FloatingBarSurfaceShape(
      topRadius: radius,
      bottomRadius: radius,
      cornerControlFactor: FloatingBarLayout.compactCornerControlFactor
    )
    let width = FloatingBarLayout.compactPillWidth(
      hovered: isBarHovered,
      showsExpand: model.liveCaptionToggleVisible
    )

    // Content is laid out at full hover width and right-aligned; the animated
    // frame + clip reveal it leftward, so the trace's older samples appear as
    // the pill grows. The stop button is trailing so it never moves and stays
    // clickable without hovering first.
    return HStack(spacing: FloatingBarLayout.compactContentSpacing) {
      ElapsedTimeText(startedAt: model.startedAt, color: primaryContentColor)
        .frame(width: FloatingBarLayout.compactTimerWidth)
        .opacity(isBarHovered ? 1 : 0)

      Group {
        if model.status == .error {
          ErrorMark(color: errorAccentColor)
            .frame(
              width: FloatingBarLayout.compactTraceWidth,
              height: FloatingBarLayout.compactTraceHeight,
              alignment: .trailing
            )
        } else {
          InkTrace(
            buffer: model.traceBuffer,
            amplitude: model.amplitude,
            inkColor: primaryContentColor,
            playheadColor: accentColor
          )
          .frame(
            width: FloatingBarLayout.compactTraceWidth,
            height: FloatingBarLayout.compactTraceHeight
          )
        }
      }

      floatingControls(isExpanded: false)
    }
    .padding(.leading, FloatingBarLayout.compactLeadingPadding)
    .padding(.trailing, FloatingBarLayout.compactTrailingPadding)
    .fixedSize(horizontal: true, vertical: false)
    .frame(width: width, height: FloatingBarLayout.compactHeight, alignment: .trailing)
    // Fade the ink out ahead of the left border so clipped trace columns
    // don't poke through the rounded edge while the pill is contracted.
    .mask(
      HStack(spacing: 0) {
        LinearGradient(
          colors: [.clear, .black],
          startPoint: .leading,
          endPoint: .trailing
        )
        .frame(width: FloatingBarLayout.compactLeadingPadding)
        Rectangle()
      }
    )
    .background(
      ZStack {
        VisualEffectBlur(colorScheme: model.colorScheme, cornerRadius: radius)
        pillShape.fill(surfaceTintColor)
      }
    )
    .overlay(
      pillShape
        .strokeBorder(outerStrokeColor, lineWidth: 0.5)
    )
    .overlay(
      pillShape
        .strokeBorder(innerStrokeColor, lineWidth: 0.5)
        .padding(1)
    )
    .clipShape(pillShape)
    .contentShape(pillShape)
    .simultaneousGesture(dragClickSuppressor)
    .onHover { isBarHovered = $0 }
    .animation(.spring(response: 0.32, dampingFraction: 0.8), value: isBarHovered)
  }

  private var expandedPanel: some View {
    let surfaceShape = FloatingBarSurfaceShape(
      topRadius: FloatingBarLayout.expandedCornerRadius,
      bottomRadius: FloatingBarLayout.expandedCornerRadius
    )

    return VStack(spacing: FloatingBarLayout.hoverHandleGap) {
      FloatingBarHoverHandle(
        color: dragHandleDotColor,
        width: FloatingBarLayout.expandedWidth
      )
      .opacity(isBarHovered ? 1 : 0)
      .scaleEffect(isBarHovered ? 1 : 0.92)
      .accessibilityHidden(true)

      ZStack(alignment: .topTrailing) {
        VStack(spacing: 12) {
          HStack {
            Text(model.title)
              .font(.system(size: 13, weight: .semibold))
              .foregroundStyle(primaryContentColor)
              .lineLimit(1)
              .truncationMode(.tail)

            Spacer(minLength: 12)
          }
          .padding(.leading, FloatingBarLayout.expandedPadding + 4)
          .padding(
            .trailing,
            FloatingBarLayout.compactControlsWidth(showsExpand: model.liveCaptionToggleVisible)
              + 12
          )
          .frame(height: FloatingBarLayout.compactHeight)

          ScrollViewReader { proxy in
            ZStack(alignment: .bottom) {
              ScrollView(.vertical, showsIndicators: false) {
                VStack(spacing: 8) {
                  ForEach(Array(model.transcriptBubbles.enumerated()), id: \.element.id) {
                    index, bubble in
                    TranscriptBubbleView(
                      bubble: bubble,
                      showsSpeakerLabel: showsSpeakerLabel(at: index),
                      colorScheme: model.colorScheme
                    )
                    .id(bubble.id)
                  }
                  Color.clear
                    .frame(height: FloatingBarLayout.expandedPadding)
                    .id(transcriptBottomAnchorId)
                }
                .frame(maxWidth: .infinity, alignment: .bottom)
                .background(
                  TranscriptScrollObserver(isPinnedToBottom: $shouldAutoScrollTranscript)
                )
              }
              .frame(maxWidth: .infinity, maxHeight: .infinity)
              .onChange(of: model.transcriptBubbles.last?.id) { _, bubbleId in
                if bubbleId != nil, shouldAutoScrollTranscript {
                  scrollTranscriptToBottom(proxy)
                }
              }
              .onAppear {
                shouldAutoScrollTranscript = true
                scrollTranscriptToBottom(proxy)
              }

              if !shouldAutoScrollTranscript, model.transcriptBubbles.last?.id != nil {
                transcriptBottomChip {
                  performClick {
                    scrollTranscriptToBottom(proxy, animated: true)
                    shouldAutoScrollTranscript = true
                  }
                }
                .padding(.bottom, 0)
                .transition(.move(edge: .bottom))
              }
            }
            .animation(.easeOut(duration: 0.12), value: shouldAutoScrollTranscript)
          }
          .padding(.horizontal, FloatingBarLayout.expandedPadding)
          .padding(.bottom, FloatingBarLayout.expandedPadding)
        }
        .frame(
          width: FloatingBarLayout.expandedWidth,
          height: FloatingBarLayout.expandedHeight,
          alignment: .top
        )

        floatingControls(isExpanded: true)
          .frame(
            width: FloatingBarLayout.compactControlsWidth(
              showsExpand: model.liveCaptionToggleVisible),
            height: FloatingBarLayout.compactHeight
          )
          .padding(.trailing, FloatingBarLayout.compactHorizontalPadding)
      }
      .frame(
        width: FloatingBarLayout.expandedWidth,
        height: FloatingBarLayout.expandedHeight,
        alignment: .top
      )
    }
    .padding(.top, FloatingBarLayout.hoverHandleTopPadding)
    .frame(
      width: FloatingBarLayout.expandedWidth,
      height: FloatingBarLayout.expandedHeight
        + (isBarHovered ? FloatingBarLayout.hoverHandleReservedHeight : 0),
      alignment: .bottom
    )
    .background(
      ZStack {
        VisualEffectBlur(
          colorScheme: model.colorScheme,
          cornerRadius: FloatingBarLayout.expandedCornerRadius
        )
        surfaceShape.fill(surfaceTintColor)
      }
    )
    .overlay(
      surfaceShape
        .strokeBorder(outerStrokeColor, lineWidth: 0.5)
    )
    .overlay(
      surfaceShape
        .strokeBorder(innerStrokeColor, lineWidth: 0.5)
        .padding(1)
    )
    .clipShape(surfaceShape)
    .contentShape(Rectangle())
    .simultaneousGesture(dragClickSuppressor)
    .onHover { isBarHovered = $0 }
    .animation(.easeOut(duration: 0.12), value: isBarHovered)
  }

  private func floatingControls(isExpanded: Bool) -> some View {
    HStack(spacing: FloatingBarLayout.compactControlGap) {
      FloatingIconButton(
        systemName: "stop.fill",
        accessibilityLabel: "Stop recording",
        color: accentColor,
        hoverFill: accentColor.opacity(0.16),
        size: FloatingBarLayout.compactControlSize,
        action: { performClick(RustBridge.stopListening) }
      )

      if model.liveCaptionToggleVisible {
        FloatingIconButton(
          systemName: isExpanded
            ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right",
          accessibilityLabel: isExpanded ? "Collapse live transcript" : "Expand live transcript",
          color: primaryContentColor,
          hoverFill: controlHoverFill,
          size: FloatingBarLayout.compactControlSize,
          action: { performClick { setExpanded(!isExpanded) } }
        )
      }
    }
  }

  private var accentColor: Color {
    model.status == .error ? errorAccentColor : normalAccentColor
  }

  private var surfaceTintColor: Color {
    if model.colorScheme == .dark {
      return Color.black.opacity(0.14)
    }

    return Color.white.opacity(0.28)
  }

  private var primaryContentColor: Color {
    if model.colorScheme == .dark {
      return .white
    }

    return Color(red: 0.12, green: 0.11, blue: 0.10)
  }

  private var controlHoverFill: Color {
    primaryContentColor.opacity(model.colorScheme == .dark ? 0.08 : 0.07)
  }

  private var outerStrokeColor: Color {
    primaryContentColor.opacity(model.colorScheme == .dark ? 0.14 : 0.12)
  }

  private var innerStrokeColor: Color {
    primaryContentColor.opacity(model.colorScheme == .dark ? 0.28 : 0.18)
  }

  private var dragHandleDotColor: Color {
    primaryContentColor.opacity(model.colorScheme == .dark ? 0.48 : 0.36)
  }

  private var errorAccentColor: Color {
    Color(red: 1, green: 0.25, blue: 0.24)
  }

  private var normalAccentColor: Color {
    if model.colorScheme == .dark {
      return Color(red: 1, green: 0.27, blue: 0.23)
    }

    return Color(red: 0.88, green: 0.21, blue: 0.17)
  }

  private var dragClickSuppressor: some Gesture {
    DragGesture(
      minimumDistance: FloatingBarLayout.dragClickThreshold,
      coordinateSpace: .global
    )
    .onChanged { _ in
      suppressNextClick = true

      let mouseLocation = NSEvent.mouseLocation
      let start =
        dragStart
        ?? panelOrigin().map {
          FloatingBarDragStart(panelOrigin: $0, mouseLocation: mouseLocation)
        }

      guard let start else { return }
      dragStart = start

      movePanel(
        NSPoint(
          x: start.panelOrigin.x + mouseLocation.x - start.mouseLocation.x,
          y: start.panelOrigin.y + mouseLocation.y - start.mouseLocation.y
        )
      )
    }
    .onEnded { _ in
      dragStart = nil
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
        suppressNextClick = false
      }
    }
  }

  private func performClick(_ action: () -> Void) {
    if suppressNextClick {
      suppressNextClick = false
      return
    }

    action()
  }

  private func setExpanded(_ expanded: Bool) {
    model.isExpanded = expanded
    settings.setLiveCaptionMinimized(!expanded)
    if !expanded {
      LiveCaptionManager.shared.hide(clearText: false)
    }
  }

  private func showsSpeakerLabel(at index: Int) -> Bool {
    guard model.transcriptBubbles.indices.contains(index) else { return false }
    guard index > model.transcriptBubbles.startIndex else { return true }

    let bubble = model.transcriptBubbles[index]
    let previousBubble = model.transcriptBubbles[index - 1]
    return bubble.speakerLabel != previousBubble.speakerLabel
      || bubble.isSelf != previousBubble.isSelf
  }

  private func scrollTranscriptToBottom(_ proxy: ScrollViewProxy, animated: Bool = false) {
    DispatchQueue.main.async {
      if animated {
        withAnimation(.easeOut(duration: 0.16)) {
          proxy.scrollTo(transcriptBottomAnchorId, anchor: .bottom)
        }
      } else {
        proxy.scrollTo(transcriptBottomAnchorId, anchor: .bottom)
      }
    }
  }

  private func transcriptBottomChip(action: @escaping () -> Void) -> some View {
    Button(action: action) {
      HStack(spacing: 6) {
        Image(systemName: "arrow.down")
          .font(.system(size: 10, weight: .semibold))
        Text("Go to bottom")
          .font(.system(size: 11, weight: .medium))
      }
      .foregroundStyle(primaryContentColor)
      .padding(.horizontal, 12)
      .padding(.vertical, 7)
      .background(
        Capsule(style: .continuous)
          .fill(transcriptChipFillColor)
      )
      .overlay(
        Capsule(style: .continuous)
          .strokeBorder(transcriptChipStrokeColor, lineWidth: 0.5)
      )
    }
    .buttonStyle(.plain)
    .accessibilityLabel("Scroll transcript to bottom")
  }

  private var transcriptChipFillColor: Color {
    if model.colorScheme == .dark {
      return Color(red: 0.18, green: 0.18, blue: 0.17)
    }

    return Color(red: 0.95, green: 0.95, blue: 0.93)
  }

  private var transcriptChipStrokeColor: Color {
    if model.colorScheme == .dark {
      return Color(red: 0.36, green: 0.36, blue: 0.34)
    }

    return Color(red: 0.76, green: 0.75, blue: 0.72)
  }
}

private struct FloatingBarDragStart {
  let panelOrigin: NSPoint
  let mouseLocation: NSPoint
}

private struct FloatingBarSurfaceShape: InsettableShape {
  let topRadius: CGFloat
  let bottomRadius: CGFloat
  let cornerControlFactor: CGFloat
  var insetAmount: CGFloat = 0

  init(
    topRadius: CGFloat,
    bottomRadius: CGFloat,
    cornerControlFactor: CGFloat = 0.447715,
    insetAmount: CGFloat = 0
  ) {
    self.topRadius = topRadius
    self.bottomRadius = bottomRadius
    self.cornerControlFactor = cornerControlFactor
    self.insetAmount = insetAmount
  }

  func path(in rect: CGRect) -> Path {
    let insetRect = rect.insetBy(dx: insetAmount, dy: insetAmount)
    let topRadius = min(topRadius, insetRect.width / 2, insetRect.height / 2)
    let bottomRadius = min(bottomRadius, insetRect.width / 2, insetRect.height / 2)
    let topControl = topRadius * cornerControlFactor
    let bottomControl = bottomRadius * cornerControlFactor
    var path = Path()

    path.move(to: CGPoint(x: insetRect.minX + topRadius, y: insetRect.minY))
    path.addLine(to: CGPoint(x: insetRect.maxX - topRadius, y: insetRect.minY))
    path.addCurve(
      to: CGPoint(x: insetRect.maxX, y: insetRect.minY + topRadius),
      control1: CGPoint(x: insetRect.maxX - topRadius + topControl, y: insetRect.minY),
      control2: CGPoint(x: insetRect.maxX, y: insetRect.minY + topRadius - topControl)
    )
    path.addLine(to: CGPoint(x: insetRect.maxX, y: insetRect.maxY - bottomRadius))
    path.addCurve(
      to: CGPoint(x: insetRect.maxX - bottomRadius, y: insetRect.maxY),
      control1: CGPoint(x: insetRect.maxX, y: insetRect.maxY - bottomRadius + bottomControl),
      control2: CGPoint(x: insetRect.maxX - bottomRadius + bottomControl, y: insetRect.maxY)
    )
    path.addLine(to: CGPoint(x: insetRect.minX + bottomRadius, y: insetRect.maxY))
    path.addCurve(
      to: CGPoint(x: insetRect.minX, y: insetRect.maxY - bottomRadius),
      control1: CGPoint(x: insetRect.minX + bottomRadius - bottomControl, y: insetRect.maxY),
      control2: CGPoint(x: insetRect.minX, y: insetRect.maxY - bottomRadius + bottomControl)
    )
    path.addLine(to: CGPoint(x: insetRect.minX, y: insetRect.minY + topRadius))
    path.addCurve(
      to: CGPoint(x: insetRect.minX + topRadius, y: insetRect.minY),
      control1: CGPoint(x: insetRect.minX, y: insetRect.minY + topRadius - topControl),
      control2: CGPoint(x: insetRect.minX + topRadius - topControl, y: insetRect.minY)
    )
    path.closeSubpath()
    return path
  }

  func inset(by amount: CGFloat) -> FloatingBarSurfaceShape {
    FloatingBarSurfaceShape(
      topRadius: topRadius,
      bottomRadius: bottomRadius,
      cornerControlFactor: cornerControlFactor,
      insetAmount: insetAmount + amount
    )
  }
}

private struct FloatingBarHoverHandle: View {
  let color: Color
  let width: CGFloat

  var body: some View {
    FloatingBarDotPattern(color: color)
      .frame(
        width: max(0, width - FloatingBarLayout.hoverHandleHorizontalPadding * 2),
        height: FloatingBarLayout.hoverHandleHeight
      )
      .padding(.horizontal, FloatingBarLayout.hoverHandleHorizontalPadding)
  }
}

private struct FloatingBarDotPattern: View {
  let color: Color

  var body: some View {
    Canvas { context, size in
      var y = FloatingBarLayout.hoverHandleDotSize / 2
      while y <= size.height {
        var x = FloatingBarLayout.hoverHandleDotSize / 2
        while x <= size.width {
          let rect = CGRect(
            x: x - FloatingBarLayout.hoverHandleDotSize / 2,
            y: y - FloatingBarLayout.hoverHandleDotSize / 2,
            width: FloatingBarLayout.hoverHandleDotSize,
            height: FloatingBarLayout.hoverHandleDotSize
          )
          context.fill(Path(ellipseIn: rect), with: .color(color))
          x += FloatingBarLayout.hoverHandleDotSpacing
        }
        y += FloatingBarLayout.hoverHandleDotSpacing
      }
    }
  }
}

private struct TranscriptScrollObserver: NSViewRepresentable {
  @Binding var isPinnedToBottom: Bool

  func makeCoordinator() -> Coordinator {
    Coordinator()
  }

  func makeNSView(context: Context) -> NSView {
    let view = NSView()
    DispatchQueue.main.async {
      context.coordinator.bind(to: view.enclosingScrollView)
    }
    return view
  }

  func updateNSView(_ view: NSView, context: Context) {
    context.coordinator.isPinnedToBottom = $isPinnedToBottom
    DispatchQueue.main.async {
      context.coordinator.bind(to: view.enclosingScrollView)
    }
  }

  final class Coordinator {
    var isPinnedToBottom: Binding<Bool>?
    private weak var scrollView: NSScrollView?
    private var boundsObserver: NSObjectProtocol?
    private let threshold: CGFloat = 20

    deinit {
      if let boundsObserver {
        NotificationCenter.default.removeObserver(boundsObserver)
      }
    }

    func bind(to scrollView: NSScrollView?) {
      guard self.scrollView !== scrollView else { return }

      if let boundsObserver {
        NotificationCenter.default.removeObserver(boundsObserver)
      }

      self.scrollView = scrollView
      guard let scrollView else { return }

      scrollView.contentView.postsBoundsChangedNotifications = true
      boundsObserver = NotificationCenter.default.addObserver(
        forName: NSView.boundsDidChangeNotification,
        object: scrollView.contentView,
        queue: .main
      ) { [weak self] _ in
        self?.updatePinnedState()
      }

      updatePinnedState()
    }

    func updatePinnedState() {
      guard let scrollView, let documentView = scrollView.documentView else { return }

      let visibleRect = scrollView.documentVisibleRect
      let documentBounds = documentView.bounds
      let isPinned: Bool
      if documentView.isFlipped {
        isPinned = visibleRect.maxY >= documentBounds.maxY - threshold
      } else {
        isPinned = visibleRect.minY <= documentBounds.minY + threshold
      }

      if isPinnedToBottom?.wrappedValue != isPinned {
        isPinnedToBottom?.wrappedValue = isPinned
      }
    }
  }
}

private struct FloatingIconButton: View {
  let systemName: String
  let accessibilityLabel: String
  let color: Color
  let hoverFill: Color
  let size: CGFloat
  let action: () -> Void
  @State private var isHovered = false

  var body: some View {
    Button(action: action) {
      Image(systemName: systemName)
        .font(.system(size: 12, weight: .semibold))
        .foregroundStyle(color)
        .frame(width: size, height: size)
        .background(
          Circle()
            .fill(isHovered ? hoverFill : Color.clear)
        )
        .contentShape(Circle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(accessibilityLabel)
    .onHover { isHovered = $0 }
  }
}

private struct TranscriptBubbleView: View {
  let bubble: FloatingTranscriptBubblePayload
  let showsSpeakerLabel: Bool
  let colorScheme: FloatingBarColorScheme

  var body: some View {
    HStack {
      if bubble.isSelf {
        Spacer(minLength: 40)
      }

      VStack(alignment: bubble.isSelf ? .trailing : .leading, spacing: 4) {
        if showsSpeakerLabel || isOverlapping {
          HStack(spacing: 4) {
            if bubble.isSelf {
              overlapGlyph
              speakerLabel
            } else {
              speakerLabel
              overlapGlyph
            }
          }
          .frame(maxWidth: .infinity, alignment: bubble.isSelf ? .trailing : .leading)
          .padding(.horizontal, 3)
        }

        let shape = RoundedRectangle(cornerRadius: 11, style: .continuous)
        Text(bubble.text)
          .font(.system(size: 13, weight: .regular))
          .foregroundStyle(Color.white)
          .multilineTextAlignment(.leading)
          .frame(maxWidth: .infinity, alignment: .leading)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.horizontal, 11)
          .padding(.vertical, 8)
          .background(shape.fill(bubbleBackground))
          .overlay(
            shape
              .strokeBorder(overlapStrokeColor, lineWidth: isOverlapping ? 1 : 0)
          )
      }

      if !bubble.isSelf {
        Spacer(minLength: 40)
      }
    }
  }

  private var bubbleBackground: Color {
    if bubble.isSelf {
      return Color.black.opacity(colorScheme == .dark ? 0.34 : 0.24)
    }

    return Color.black.opacity(colorScheme == .dark ? 0.28 : 0.2)
  }

  private var speakerLabel: some View {
    Group {
      if showsSpeakerLabel {
        Text(bubble.speakerLabel)
          .font(.system(size: 10, weight: .semibold))
          .foregroundStyle(Color.white)
          .lineLimit(1)
      }
    }
  }

  private var overlapGlyph: some View {
    Group {
      if isOverlapping {
        Image(systemName: "arrow.left.and.right")
          .font(.system(size: 8, weight: .bold))
          .foregroundStyle(Color.white.opacity(0.72))
          .frame(width: 12, height: 12)
          .accessibilityLabel("Overlapping speech")
      }
    }
  }

  private var isOverlapping: Bool {
    bubble.overlapsPrevious || bubble.overlapsNext
  }

  private var overlapStrokeColor: Color {
    Color.white.opacity(colorScheme == .dark ? 0.26 : 0.34)
  }
}

private struct ErrorMark: View {
  let color: Color

  var body: some View {
    VStack(spacing: 1.5) {
      Capsule(style: .continuous)
        .fill(color)
        .frame(width: 3.2, height: 8)
      Circle()
        .fill(color)
        .frame(width: 3.2, height: 3.2)
    }
  }
}

final class InkTraceBuffer {
  static let capacity = 110
  static let sampleInterval: TimeInterval = 1.0 / 30.0

  private var samples = [Double](repeating: 0, count: InkTraceBuffer.capacity)
  private var head = 0
  private var lastSampleTime: TimeInterval = 0

  func push(_ value: Double, at time: TimeInterval) {
    guard time - lastSampleTime >= Self.sampleInterval else { return }
    lastSampleTime = time
    samples[head] = value
    head = (head + 1) % Self.capacity
  }

  func newestFirst() -> [Double] {
    var ordered = [Double](repeating: 0, count: Self.capacity)
    for index in 0..<Self.capacity {
      ordered[index] = samples[(head - 1 - index + Self.capacity * 2) % Self.capacity]
    }
    return ordered
  }
}

private struct InkTrace: View {
  let buffer: InkTraceBuffer
  let amplitude: Double
  let inkColor: Color
  let playheadColor: Color

  var body: some View {
    TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: false)) { timeline in
      Canvas { context, size in
        let clamped = min(max(amplitude, 0), 1)
        buffer.push(clamped, at: timeline.date.timeIntervalSinceReferenceDate)

        let samples = buffer.newestFirst()
        let mid = size.height / 2
        let playheadX = size.width - 2.5
        let maxHalf = mid * 0.92

        for (age, sample) in samples.enumerated() {
          let x = playheadX - 3 - CGFloat(age)
          guard x >= 1 else { break }
          let half = max(0.9, CGFloat(Self.boosted(sample)) * maxHalf)
          let alpha = 0.9 - 0.65 * Double(age) / Double(InkTraceBuffer.capacity - 1)
          var column = Path()
          column.move(to: CGPoint(x: x, y: mid - half))
          column.addLine(to: CGPoint(x: x, y: mid + half))
          context.stroke(
            column,
            with: .color(inkColor.opacity(alpha)),
            style: StrokeStyle(lineWidth: 1.2, lineCap: .round)
          )
        }

        let radius = 1.7 + CGFloat(Self.boosted(clamped)) * 1.3
        context.fill(
          Path(
            ellipseIn: CGRect(
              x: playheadX - radius,
              y: mid - radius,
              width: radius * 2,
              height: radius * 2
            )),
          with: .color(playheadColor)
        )
      }
    }
  }

  // Raw amplitude sits around 0.05-0.3 for normal speech; compress the range
  // so speech reads clearly while true silence still draws flat.
  private static func boosted(_ value: Double) -> Double {
    let gated = max(0, value - 0.015) / 0.985
    return min(1, pow(gated, 0.4))
  }
}

private struct ElapsedTimeText: View {
  let startedAt: Date?
  let color: Color

  var body: some View {
    TimelineView(.periodic(from: .now, by: 1)) { timeline in
      Text(Self.formatted(from: startedAt, to: timeline.date))
        .font(.system(size: 11.5, weight: .medium).monospacedDigit())
        .foregroundStyle(color)
        .lineLimit(1)
    }
  }

  private static func formatted(from start: Date?, to now: Date) -> String {
    let total = start.map { max(0, Int(now.timeIntervalSince($0))) } ?? 0
    let hours = total / 3600
    let minutes = (total % 3600) / 60
    let seconds = total % 60
    if hours > 0 {
      return String(format: "%d:%02d:%02d", hours, minutes, seconds)
    }
    return String(format: "%d:%02d", minutes, seconds)
  }
}

private struct VisualEffectBlur: NSViewRepresentable {
  let colorScheme: FloatingBarColorScheme
  let cornerRadius: CGFloat

  func makeNSView(context: Context) -> NSVisualEffectView {
    let view = NSVisualEffectView()
    view.blendingMode = .behindWindow
    view.material = .hudWindow
    view.state = .active
    view.maskImage = Self.maskImage(cornerRadius: cornerRadius)
    return view
  }

  func updateNSView(_ view: NSVisualEffectView, context: Context) {
    view.appearance = NSAppearance(named: colorScheme == .dark ? .darkAqua : .aqua)
  }

  // behindWindow blur ignores layer masks; maskImage is the supported clip.
  private static func maskImage(cornerRadius: CGFloat) -> NSImage {
    let edge = cornerRadius * 2 + 1
    let image = NSImage(size: NSSize(width: edge, height: edge), flipped: false) { rect in
      NSColor.black.setFill()
      NSBezierPath(roundedRect: rect, xRadius: cornerRadius, yRadius: cornerRadius).fill()
      return true
    }
    image.capInsets = NSEdgeInsets(
      top: cornerRadius, left: cornerRadius, bottom: cornerRadius, right: cornerRadius)
    image.resizingMode = .stretch
    return image
  }
}
