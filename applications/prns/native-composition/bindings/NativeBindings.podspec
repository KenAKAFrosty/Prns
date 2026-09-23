# Application-owned JSI2 packaging, initialized from uniffi-bindgen-react-native.
require "json"

package = JSON.parse(File.read(File.join(__dir__, "package.json")))

Pod::Spec.new do |s|
  s.name         = "NativeBindings"
  s.version      = package["version"]
  s.summary      = package["description"]
  s.homepage     = package["homepage"]
  s.license      = package["license"]
  s.authors      = package["author"]

  s.platforms    = { :ios => min_ios_version_supported }
  s.source       = { :git => "https://github.com/KenAKAFrosty/Prns.git", :tag => "#{s.version}" }

  # Product namespaces share the SDK-selected Rust image.
  s.dependency "PrnsHostExpo"
  s.dependency "UbjsReactNative"
end
