require 'json'

package = JSON.parse(File.read(File.join(__dir__, '..', 'package.json')))

Pod::Spec.new do |s|
  s.name             = 'PrnsApp'
  s.version          = package['version']
  s.summary          = package['description']
  s.description      = package['description']
  s.license          = package['license']
  s.author           = 'Prns contributors'
  s.homepage         = 'https://reticulum.rs'
  s.source           = { :git => 'https://github.com/KenAKAFrosty/Prns.git', :tag => s.version.to_s }
  s.platforms        = { :ios => '18.0' }
  s.swift_version    = '5.9'
  s.static_framework = true

  s.dependency 'ExpoModulesCore'
  s.dependency 'NativeBindings'

  s.source_files = '**/*.{h,swift}'
  s.public_header_files = 'generated/PrnsAppBindingsFFI.h', 'PrnsNativeDiagnostics.h'
  s.frameworks = 'AccessorySetupKit', 'CoreBluetooth', 'CoreFoundation', 'Foundation', 'UIKit'
  s.libraries = 'iconv'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
end
