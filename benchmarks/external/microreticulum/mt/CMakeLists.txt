cmake_minimum_required(VERSION 3.15)
project(mt_mr LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release)
endif()

set(RNS_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(RNS_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(RNS_BUILD_INTEROP OFF CACHE BOOL "" FORCE)

find_package(Threads REQUIRED)
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../.upstream ${CMAKE_BINARY_DIR}/mr-build)

add_executable(mr_announce_mt main.cpp)
target_link_libraries(mr_announce_mt PRIVATE microReticulum Threads::Threads)
