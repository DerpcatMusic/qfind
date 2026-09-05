cmake_minimum_required(VERSION 3.16)
project(qfind-qt LANGUAGES CXX)

# Keep the incumbent KDE/Breeze build path while sharing the canonical Qt app.
add_subdirectory("${CMAKE_CURRENT_LIST_DIR}/../../apps/qt" qfind-qt-source)
