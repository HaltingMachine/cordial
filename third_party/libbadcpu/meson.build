libbadcpu_inc = include_directories('include')

libbadcpu_src = files(
  'src/badcpu.cpp',
  'src/cpuid.cpp',
  'src/decoder.cpp',
  'src/emulator.cpp',
  'src/signal_handler.cpp',
)

libbadcpu = shared_library('badcpu',
  libbadcpu_src,
  include_directories: libbadcpu_inc,
  dependencies: dependency('threads'),
  install: true,
)

libbadcpu_dep = declare_dependency(
  link_with: libbadcpu,
  include_directories: libbadcpu_inc,
)

test_badcpu = executable('test_badcpu',
  'test/test_badcpu.cpp',
  include_directories: libbadcpu_inc,
  link_with: libbadcpu,
)

test('badcpu-test', test_badcpu)
