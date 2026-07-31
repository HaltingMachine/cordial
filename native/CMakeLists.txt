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
add_library(cordial_linker_shim STATIC shim.cpp)
target_link_libraries(cordial_linker_shim PUBLIC linker)

install(TARGETS cordial_linker_shim linker ARCHIVE DESTINATION lib)
