require 'json'

root = File.expand_path('..', __dir__)
package = JSON.parse(File.read(File.join(root, 'package.json')))
selection = JSON.parse(File.read(File.join(root, 'native-image.json')))
image = selection.fetch('libraryName')
raise 'Invalid PRNS image name' unless image.match?(/\A[a-z][a-z0-9_]*\z/)
frameworks = Dir.glob(File.join(__dir__, '*.xcframework'))
expected = File.join(__dir__, "#{image}.xcframework")
raise "PRNS requires exactly one selected framework: #{expected}; build the selected provider first" unless frameworks == [expected]

Pod::Spec.new do |s|
  s.name = 'PrnsHostExpo'
  s.version = package['version']
  s.summary = package['description']
  s.license = package['license']
  s.author = 'Prns contributors'
  s.homepage = 'https://reticulum.rs'
  s.source = { :git => 'https://github.com/KenAKAFrosty/Prns.git', :tag => s.version.to_s }
  s.platforms = { :ios => '18.0' }
  s.swift_version = '5.9'
  s.frameworks = 'Foundation', 'UIKit', 'CoreBluetooth'
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  s.dependency 'UbjsReactNative'
  s.source_files = '*.swift', 'generated/*.{h,swift}'
  s.public_header_files = 'generated/*.h'
  s.vendored_frameworks = "#{image}.xcframework"
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES', 'SWIFT_COMPILATION_MODE' => 'wholemodule' }
end
