@echo off
REM Set up VS 2022 MSVC compiler (cl.exe, link.exe, etc.)
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"

REM Add VS 2019's bundled cmake and ninja to PATH.
REM These are needed because rusty_link builds Ableton Link via cmake and
REM .cargo/config.toml sets CMAKE_GENERATOR=Ninja.  VS 2022/2026 do not ship
REM cmake as a component on this machine, so we borrow the VS 2019 copies.
REM A standalone cmake install (winget install Kitware.CMake) is a cleaner
REM alternative that can be removed from here once in place.
set VS19_CMAKE=C:\Program Files (x86)\Microsoft Visual Studio\2019\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin
set VS19_NINJA=C:\Program Files (x86)\Microsoft Visual Studio\2019\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja
set PATH=%VS19_CMAKE%;%VS19_NINJA%;%PATH%

cd /d "C:\Users\andre\Developer\rust\rustjay-engine"
cargo run -p template 2>&1
