// swift-tools-version:5.3
// Chirper's native iOS code. Built by `cargo tauri ios build` on a macOS runner with full Xcode.

import PackageDescription

let package = Package(
  name: "tauri-plugin-chirper-native",
  platforms: [
    .macOS(.v10_13),
    .iOS(.v15),
  ],
  products: [
    .library(
      name: "tauri-plugin-chirper-native",
      type: .static,
      targets: ["tauri-plugin-chirper-native"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-chirper-native",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
