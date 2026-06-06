cmake_minimum_required(VERSION 3.15)
project(mr_announce_bench LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release)
endif()

# Don't build microReticulum's own tests/examples/interop (and skip their fetches).
set(RNS_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(RNS_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(RNS_BUILD_INTEROP OFF CACHE BOOL "" FORCE)

# The pinned upstream, cloned by run.sh.
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/.upstream
                 ${CMAKE_BINARY_DIR}/microReticulum-build)

add_executable(mr_announce_bench main.cpp)
target_link_libraries(mr_announce_bench PRIVATE microReticulum)
