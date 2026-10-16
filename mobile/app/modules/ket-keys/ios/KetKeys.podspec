Pod::Spec.new do |s|
  s.name           = 'KetKeys'
  s.version        = '0.1.0'
  s.summary        = 'Hardware keyboard shortcuts for the ket app.'
  s.description    = 'A view that adds keyboard shortcuts to whatever is focused inside it.'
  s.license        = 'MIT'
  s.author         = 'ket'
  s.homepage       = 'https://ketapp.dev'
  s.platforms      = { :ios => '16.4' }
  s.swift_version  = '5.9'
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.source_files = '**/*.{h,m,swift}'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
end
