project('cordial', 'c', 'cpp',
  version: '0.0.1',
  default_options: [
    'warning_level=2',
    'c_std=c17',
    'cpp_std=c++20',
  ],
  license: 'MIT',
)

# Phase 1 runtime components. Only the vendored CPU-feature emulator exists so
# far; the ELF loader, bionic shim, syscall translation, graphics, audio and
# input layers are not written yet. See docs/findings.md.
subdir('third_party/libbadcpu')
