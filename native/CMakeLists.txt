cmake_minimum_required(VERSION 3.16)
project(cordial_native LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# Clang is not a preference. AOSP bionic uses C11 _Atomic inside C++ headers
# (bionic_elf_tls.h, bionic_fdsan.h) — a Clang extension GCC rejects, producing
# 144 errors on an otherwise clean tree. See docs/base-evaluation.md §2.1.
if(NOT CMAKE_CXX_COMPILER_ID MATCHES "Clang")
  message(FATAL_ERROR
    "The native subtree requires Clang (found ${CMAKE_CXX_COMPILER_ID}). "
    "AOSP bionic does not build with GCC.")
endif()

# The AOSP bionic linker, via minecraft-linux's host wrapper. Target: `linker`.
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../third_party/mcpelauncher-linker
                 ${CMAKE_CURRENT_BINARY_DIR}/mcpelauncher-linker)

# Thin extern "C" surface over the linker's C++ API, so Rust can drive it.
# Android liblog. Roblox narrates its own startup through it, so this is the
# runtime layer's most valuable diagnostic — and the source of onLogLine/onJoin.
add_library(cordial_liblog STATIC liblog.cpp)

add_library(cordial_linker_shim STATIC shim.cpp)
target_link_libraries(cordial_linker_shim PUBLIC linker)

install(TARGETS cordial_linker_shim cordial_liblog linker ARCHIVE DESTINATION lib)

# Vendored x86-64 CPU feature emulator (see third_party/libbadcpu/README.md).
# Independent of the bionic subtree; built here so there is one native build.
add_library(badcpu STATIC
  ../third_party/libbadcpu/src/badcpu.cpp
  ../third_party/libbadcpu/src/cpuid.cpp
  ../third_party/libbadcpu/src/decoder.cpp
  ../third_party/libbadcpu/src/emulator.cpp
  ../third_party/libbadcpu/src/signal_handler.cpp
)
target_include_directories(badcpu PUBLIC ../third_party/libbadcpu/include)
install(TARGETS badcpu ARCHIVE DESTINATION lib)

# JNI virtual machine. Roblox's native code calls into Java through 518 statically
# registered natives and expects a JavaVM to call back into; libjnivm supplies both
# sides without a JVM. Debug mode is what makes VM::GenerateClassDump available —
# it records the Java classes and methods Roblox actually reaches for, which is the
# only reliable way to turn the framework-API inventory into a work queue.
set(JNIVM_ENABLE_DEBUG ON CACHE BOOL "" FORCE)
set(JNIVM_ENABLE_TRACE ON CACHE BOOL "" FORCE)
set(JNIVM_ENABLE_TESTS OFF CACHE BOOL "" FORCE)
set(JNIVM_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../third_party/libjnivm
                 ${CMAKE_CURRENT_BINARY_DIR}/libjnivm EXCLUDE_FROM_ALL)

add_library(cordial_jni_shim STATIC jni_shim.cpp android_classes.cpp game_activity.cpp init_params.cpp)
target_link_libraries(cordial_jni_shim PUBLIC jnivm)

install(TARGETS cordial_jni_shim jnivm ARCHIVE DESTINATION lib)
