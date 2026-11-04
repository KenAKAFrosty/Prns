require 'json'
package = JSON.parse(File.read(File.join(__dir__, '..', 'package.json')))
Pod::Spec.new do |s|
  s.name = 'PrnsAppPlatform'
  s.version = package['version']
  s.summary = package['description']
  s.license = package['license']
  s.author = 'Prns contributors'
  s.homepage = 'https://reticulum.rs'
  s.source = { :git => 'https://github.com/KenAKAFrosty/Prns.git' }
  s.platforms = { :ios => '18.0' }
  s.swift_version = '5.9'
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  # The selected SDK owns every generated namespace and the only native image.
  s.dependency 'PrnsHostExpo'
  s.source_files = '*.swift'
  s.frameworks = 'Foundation', 'UIKit', 'CoreBluetooth'
end
