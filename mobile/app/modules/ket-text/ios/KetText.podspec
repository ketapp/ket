Pod::Spec.new do |s|
  s.name           = 'KetText'
  s.version        = '0.1.0'
  s.summary        = 'How the ket app draws code inside its text.'
  s.description    = 'Draws the fill behind inline code as rounded chips the height of its letters.'
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
