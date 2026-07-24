// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "PersonalRns",
    platforms: [
        .macOS(.v11),
    ],
    products: [
        .library(name: "PersonalRns", targets: ["PersonalRns"]),
    ],
    targets: [
        .systemLibrary(
            name: "CPrnsHost",
            path: "prns-host/bindings/swift/Sources/CPrnsHost",
            pkgConfig: "personal-rns"
        ),
        .target(
            name: "PersonalRns",
            dependencies: ["CPrnsHost"],
            path: "prns-host/bindings/swift/Sources/PersonalRns"
        ),
        .testTarget(
            name: "PersonalRnsTests",
            dependencies: ["PersonalRns"],
            path: "prns-host/bindings/swift/Tests/PersonalRnsTests"
        ),
    ]
)
