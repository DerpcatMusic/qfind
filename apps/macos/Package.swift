// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "QfindMac",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "Qfind", targets: ["QfindMac"])],
    targets: [
        .systemLibrary(name: "CQfind", path: "Sources/CQfind"),
        .executableTarget(
            name: "QfindMac",
            dependencies: ["CQfind"],
            path: "Sources/QfindMac",
            linkerSettings: [.unsafeFlags(["-L", "../../target/release", "-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"])]
        ),
    ]
)
