@echo off
REM Build without vcvars: the sandbox blocks reg.exe, which vcvars64.bat needs.
REM Instead we point LIB/INCLUDE/PATH straight at the MSVC + WinSDK folders.
set MSVC=C:\Program Files\Microsoft Visual Studio\18\Community\VC\Tools\MSVC\14.51.36231
set W10=C:\Program Files (x86)\Windows Kits\10
set PATH=%MSVC%\bin\Hostx64\x64;%USERPROFILE%\.cargo\bin;%PATH%
set LIB=%MSVC%\lib\x64;%W10%\Lib\10.0.26100.0\ucrt\x64;%W10%\Lib\10.0.26100.0\um\x64
set INCLUDE=%MSVC%\include;%W10%\Include\10.0.26100.0\ucrt;%W10%\Include\10.0.26100.0\um;%W10%\Include\10.0.26100.0\shared;%W10%\Include\10.0.26100.0\winrt;%W10%\Include\10.0.26100.0\cppwinrt
cd /d "C:\Users\Administrator\WorkBuddy\2026-10-13-18-00-22\mfloy_launchpad"
echo === cargo build --release ===
cargo build --release 2>&1
echo === EXIT %ERRORLEVEL% ===
