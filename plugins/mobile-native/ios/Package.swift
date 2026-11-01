// swift-tools-version: 6.0
import PackageDescription

let package = Package(
  name: "tauri-plugin-mobile-native",
  platforms: [.iOS("26.0"), .macOS(.v14)],
  products: [
    .library(
      name: "tauri-plugin-mobile-native", type: .static, targets: ["tauri-plugin-mobile-native"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api"),
    .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.15.5"),
  ],
  targets: [
    .target(
      name: "tauri-plugin-mobile-native",
      dependencies: [.byName(name: "Tauri"), .product(name: "FluidAudio", package: "FluidAudio")],
      path: "Sources"
    ),
    .testTarget(
      name: "NativeAudioTests",
      dependencies: ["tauri-plugin-mobile-native"],
      path: "Tests/NativeAudioTests"
    ),
  ],
  swiftLanguageModes: [.v5]
)
