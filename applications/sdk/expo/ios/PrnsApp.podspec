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
  s.platforms        = { :ios => '16.4' }
  s.swift_version    = '5.9'
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.source_files = '**/*.{h,swift}'
  s.public_header_files = 'PrnsAppBridge.h'
  s.frameworks = 'CoreBluetooth', 'CoreFoundation', 'Foundation', 'UIKit'
  s.libraries = 'iconv'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'HEADER_SEARCH_PATHS' => '$(inherited) "${PODS_TARGET_SRCROOT}/../../../prns/native-composition/include"',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
  s.user_target_xcconfig = {
    'HEADER_SEARCH_PATHS' => '$(inherited) "${PODS_ROOT}/../../../native-composition/include"',
    'LIBRARY_SEARCH_PATHS' => '$(inherited) "${PODS_CONFIGURATION_BUILD_DIR}"',
    'OTHER_LDFLAGS' => '$(inherited) -lprns_app'
  }

  script_phase = {
    :name => 'Build prns app Rust static library',
    :script => 'PRNS_APP_ARCHIVE_OUTPUT="${PODS_CONFIGURATION_BUILD_DIR}/libprns_app.a" bash "${PODS_TARGET_SRCROOT}/build-rust.sh"',
    :execution_position => :before_compile
  }
  if Gem::Version.new(Pod::VERSION) >= Gem::Version.new('1.13.0')
    script_phase[:always_out_of_date] = '1'
  end
  s.script_phase = script_phase
end
