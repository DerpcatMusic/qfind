cmake_minimum_required(VERSION 3.16)
project(qfind-qt VERSION 0.3.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(QFIND_SOURCE_ROOT "${CMAKE_CURRENT_LIST_DIR}/../..")

find_package(Qt6 QUIET COMPONENTS Widgets Concurrent)
if(Qt6_FOUND)
    set(QT_PACKAGE Qt6)
else()
    find_package(Qt5 REQUIRED COMPONENTS Widgets Concurrent)
    set(QT_PACKAGE Qt5)
endif()

set(QFIND_NATIVE_RUNTIME "${QFIND_NATIVE_LIBRARY}")
set(QFIND_NATIVE_IMPLIB "")
if(WIN32)
    if(NOT QFIND_NATIVE_LIBRARY)
        find_file(QFIND_NATIVE_IMPLIB
            NAMES qfind_native.dll.lib libqfind_native.dll.a qfind_native.lib
            PATHS "${QFIND_SOURCE_ROOT}/target/release"
                  "${QFIND_SOURCE_ROOT}/target/debug"
            NO_DEFAULT_PATH
        )
        if(QFIND_NATIVE_IMPLIB)
            get_filename_component(QFIND_NATIVE_DIR "${QFIND_NATIVE_IMPLIB}" DIRECTORY)
            find_file(QFIND_NATIVE_RUNTIME
                NAMES qfind_native.dll
                PATHS "${QFIND_NATIVE_DIR}"
                NO_DEFAULT_PATH
            )
        endif()
    else()
        get_filename_component(QFIND_NATIVE_DIR "${QFIND_NATIVE_LIBRARY}" DIRECTORY)
        if(QFIND_NATIVE_LIBRARY MATCHES "\\.(dll|DLL)$")
            set(QFIND_NATIVE_RUNTIME "${QFIND_NATIVE_LIBRARY}")
            find_file(QFIND_NATIVE_IMPLIB
                NAMES qfind_native.dll.lib libqfind_native.dll.a qfind_native.lib
                PATHS "${QFIND_NATIVE_DIR}"
                NO_DEFAULT_PATH
            )
        else()
            set(QFIND_NATIVE_IMPLIB "${QFIND_NATIVE_LIBRARY}")
            find_file(QFIND_NATIVE_RUNTIME
                NAMES qfind_native.dll
                PATHS "${QFIND_NATIVE_DIR}"
                NO_DEFAULT_PATH
            )
        endif()
    endif()
    if(NOT QFIND_NATIVE_RUNTIME OR NOT QFIND_NATIVE_IMPLIB)
        message(FATAL_ERROR "Set QFIND_NATIVE_LIBRARY to qfind-native.dll or its import library")
    endif()
else()
    if(NOT QFIND_NATIVE_LIBRARY)
        find_library(QFIND_NATIVE_LIBRARY
            NAMES qfind_native
            PATHS "${QFIND_SOURCE_ROOT}/target/release"
                  "${QFIND_SOURCE_ROOT}/target/debug"
            NO_DEFAULT_PATH
        )
        set(QFIND_NATIVE_RUNTIME "${QFIND_NATIVE_LIBRARY}")
    endif()
    if(NOT QFIND_NATIVE_RUNTIME)
        message(FATAL_ERROR "Set QFIND_NATIVE_LIBRARY to the qfind-native shared library")
    endif()
endif()

add_executable(qfind-qt main.cpp)
set_target_properties(qfind-qt PROPERTIES RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}")
target_include_directories(qfind-qt PRIVATE "${QFIND_SOURCE_ROOT}/crates/native/include")
add_library(qfind-native SHARED IMPORTED GLOBAL)
set_target_properties(qfind-native PROPERTIES
    IMPORTED_LOCATION "${QFIND_NATIVE_RUNTIME}"
    IMPORTED_NO_SONAME TRUE
)
if(WIN32)
    set_target_properties(qfind-native PROPERTIES IMPORTED_IMPLIB "${QFIND_NATIVE_IMPLIB}")
endif()
target_link_libraries(qfind-qt PRIVATE "${QT_PACKAGE}::Widgets" "${QT_PACKAGE}::Concurrent" qfind-native)

if(APPLE)
    set_target_properties(qfind-qt PROPERTIES
        MACOSX_BUNDLE TRUE
        BUILD_RPATH "@executable_path/../Frameworks"
        INSTALL_RPATH "@executable_path/../Frameworks"
    )
elseif(UNIX)
    set_target_properties(qfind-qt PROPERTIES BUILD_RPATH "$ORIGIN" INSTALL_RPATH "$ORIGIN")
endif()

install(TARGETS qfind-qt
    BUNDLE DESTINATION .
    RUNTIME DESTINATION bin
)

if(WIN32)
    install(FILES "${QFIND_NATIVE_RUNTIME}" DESTINATION bin)
endif()
