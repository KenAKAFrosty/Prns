// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "PersonalRns",
    platforms: [
        .macOS(.v15),
    ],
    products: [
        .library(name: "PersonalRns", targets: ["PersonalRns"]),
    ],
    targets: [
        .systemLibrary(
            name: "CPrnsHost",
            pkgConfig: "personal-rns"
        ),
        .target(
            name: "PersonalRns",
            dependencies: ["CPrnsHost"]
        ),
        .testTarget(
            name: "PersonalRnsTests",
            dependencies: ["PersonalRns"]
        ),
    ]
)
