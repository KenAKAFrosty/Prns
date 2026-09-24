from enum import Enum


class HostPlatform(Enum):
    LINUX_AMD64 = "linux-amd64"
    LINUX_ARM64 = "linux-arm64"
    MACOS_AMD64 = "macos-amd64"
    MACOS_ARM64 = "macos-arm64"
    WINDOWS_AMD64 = "windows-amd64"
