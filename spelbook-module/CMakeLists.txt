cmake_minimum_required(VERSION 3.16)
project(SpelbookModule VERSION 1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 REQUIRED COMPONENTS Core Qml Quick QuickWidgets)

# ── Rust FFI shared library ───────────────────────────────────────────────────

add_library(lez_registry_ffi SHARED IMPORTED)
set_target_properties(lez_registry_ffi PROPERTIES
    IMPORTED_LOCATION "${CMAKE_CURRENT_SOURCE_DIR}/../target/release/liblez_registry_ffi.so"
)

# ── Spelbook Qt plugin ────────────────────────────────────────────────────────

add_library(SpelbookPlugin SHARED
    src/SpelbookBackend.h
    src/SpelbookPlugin.cpp
)
target_include_directories(SpelbookPlugin PRIVATE src)
target_link_libraries(SpelbookPlugin PRIVATE
    Qt6::Core
    Qt6::Qml
    Qt6::Quick
    Qt6::QuickWidgets
    lez_registry_ffi
)
set_target_properties(SpelbookPlugin PROPERTIES
    OUTPUT_NAME "spelbook_plugin"
    BUILD_RPATH "${CMAKE_CURRENT_SOURCE_DIR}/../target/release"
)

# ── Standalone test executable ────────────────────────────────────────────────

add_executable(SpelbookApp src/SpelbookMain.cpp)
target_include_directories(SpelbookApp PRIVATE src)
target_link_libraries(SpelbookApp PRIVATE
    Qt6::Core
    Qt6::Qml
    Qt6::Quick
    Qt6::QuickWidgets
    lez_registry_ffi
)
set_target_properties(SpelbookApp PROPERTIES
    BUILD_RPATH "${CMAKE_CURRENT_SOURCE_DIR}/../target/release"
)
